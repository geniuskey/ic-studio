//! MCP tools for the PCell Designer: make a parametric cell from a drawn layout, or draw one from
//! nothing, edit its parameters and operations, check it at its parameter extremes and export it
//! to Virtuoso. The template lives in its master cell (`lib/<name>`), whose layout is the PCell at
//! its defaults; every edit stores it again and regenerates the variants already placed.

use ics_core::pcell_design::{Axis, Level, Op, Param, ParamKind, Side, Target, Template};

use super::*;

/// Suggestions in the order they apply.
const SUGGESTIONS: [&str; 4] = ["contacts", "fingers", "width", "length"];

fn template_of<'p>(p: &'p Project, r: &CellRef) -> Result<&'p Template, String> {
    match p.cell(r).and_then(|c| c.pcell.as_ref()) {
        Some(PCell::Template { def, .. }) if r.lib != PCELL_LIB => Ok(def),
        Some(PCell::Template { .. }) => {
            Err(format!("{r} is a placed variant; use its master (project_info lists pcell_designs)"))
        }
        _ => Err(format!("{r} is not a PCell Designer master; make one with pcell_design_create")),
    }
}

fn rect_arg(t: &Tech, v: &Value, what: &str) -> Result<IRect, String> {
    match v.as_array().map(|a| a.iter().map(Value::as_f64).collect::<Vec<_>>()).as_deref() {
        Some([Some(x0), Some(y0), Some(x1), Some(y1)]) => {
            let r = IRect::new(dbu(t, *x0), dbu(t, *y0), dbu(t, *x1), dbu(t, *y1));
            if r.width() == 0 || r.height() == 0 {
                return Err(format!("{what} has zero width or height after snapping to the grid"));
            }
            Ok(r)
        }
        _ => Err(format!("{what} must be [x0, y0, x1, y1] in µm")),
    }
}

fn points_arg(t: &Tech, v: &Value, what: &str) -> Result<Vec<(i64, i64)>, String> {
    let a = v.as_array().ok_or_else(|| format!("{what} must be an array of [x, y] in µm"))?;
    a.iter()
        .map(|p| match p.as_array().map(|v| v.iter().map(Value::as_f64).collect::<Vec<_>>()).as_deref() {
            Some([Some(x), Some(y)]) => Ok((dbu(t, *x), dbu(t, *y))),
            _ => Err(format!("invalid point {p} in {what}; expected [x, y]")),
        })
        .collect()
}

/// An expression given as a string or a number.
fn expr_arg(o: &Map<String, Value>, k: &str, op: &str) -> Result<String, String> {
    match o.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => Err(format!("{op}: '{k}' must be an expression (string, µm) or a number")),
    }
}

fn layers_arg(t: &Tech, o: &Map<String, Value>) -> Result<Vec<usize>, String> {
    match o.get("layers") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().ok_or_else(|| "layers must be layer names".to_string()).and_then(|n| layer_index(t, n)))
            .collect(),
        Some(_) => Err("layers must be an array of layer names".into()),
    }
}

fn axis_arg(o: &Map<String, Value>, op: &str) -> Result<Axis, String> {
    match o.get("axis").and_then(Value::as_str) {
        Some("x") => Ok(Axis::X),
        Some("y") => Ok(Axis::Y),
        _ => Err(format!("{op}: 'axis' must be \"x\" or \"y\"")),
    }
}

fn op_from_json(t: &Tech, v: &Value) -> Result<Op, String> {
    let o = v.as_object().ok_or("each op must be an object")?;
    let kind = o.get("op").and_then(Value::as_str).ok_or("each op needs 'op': stretch, repeat, condition or contact_fill")?;
    let num = |k: &str| o.get(k).and_then(Value::as_f64).ok_or_else(|| format!("{kind}: '{k}' must be a number (µm)"));
    Ok(match kind {
        "stretch" => Op::Stretch {
            axis: axis_arg(o, kind)?,
            at: dbu(t, num("at")?),
            side: match o.get("side").and_then(Value::as_str).unwrap_or("high") {
                "high" => Side::High,
                "low" => Side::Low,
                "both" => Side::Both,
                s => return Err(format!("stretch: unknown side '{s}' (use high, low or both)")),
            },
            amount: expr_arg(o, "amount", kind)?,
            layers: layers_arg(t, o)?,
        },
        "repeat" => Op::Repeat {
            axis: axis_arg(o, kind)?,
            region: rect_arg(t, o.get("region").unwrap_or(&Value::Null), "repeat region")?,
            count: expr_arg(o, "count", kind)?,
            pitch: expr_arg(o, "pitch", kind)?,
            at: match o.get("at") {
                None | Some(Value::Null) => None,
                Some(_) => Some(dbu(t, num("at")?)),
            },
            layers: layers_arg(t, o)?,
        },
        "condition" => Op::Condition {
            region: rect_arg(t, o.get("region").unwrap_or(&Value::Null), "condition region")?,
            when: expr_arg(o, "when", kind)?,
            layers: layers_arg(t, o)?,
        },
        "contact_fill" => {
            let via = o.get("via").and_then(Value::as_str).ok_or("contact_fill: 'via' must be a via name")?;
            if !t.vias.iter().any(|d| d.name == via) {
                let names: Vec<&str> = t.vias.iter().map(|d| d.name.as_str()).collect();
                return Err(format!("unknown via '{via}'; vias: {}", names.join(", ")));
            }
            Op::ContactFill { rect: rect_arg(t, o.get("rect").unwrap_or(&Value::Null), "contact_fill rect")?, via: via.into() }
        }
        k => return Err(format!("unknown op '{k}' (use stretch, repeat, condition or contact_fill)")),
    })
}

fn op_json(t: &Tech, k: usize, op: &Op) -> Value {
    let layers = |l: &[usize]| l.iter().map(|&i| layer_name(t, i)).collect::<Vec<_>>();
    let mut v = match op {
        Op::Stretch { axis, at, side, amount, layers: l } => json!({
            "op": "stretch", "axis": axis, "at": um(t, *at), "side": side, "amount": amount, "layers": layers(l),
        }),
        Op::Repeat { axis, region, count, pitch, at, layers: l } => json!({
            "op": "repeat", "axis": axis, "region": rect_um(t, *region), "count": count, "pitch": pitch,
            "at": at.map(|a| um(t, a)), "layers": layers(l),
        }),
        Op::Condition { region, when, layers: l } => json!({
            "op": "condition", "region": rect_um(t, *region), "when": when, "layers": layers(l),
        }),
        Op::ContactFill { rect, via } => json!({"op": "contact_fill", "rect": rect_um(t, *rect), "via": via}),
    };
    v["index"] = json!(k);
    v["summary"] = json!(op.summary(t.dbu_per_um));
    v
}

fn kind_arg(v: &Value) -> Result<ParamKind, String> {
    match v.as_str() {
        Some("length") => Ok(ParamKind::Length),
        Some("int" | "integer") => Ok(ParamKind::Int),
        Some("bool" | "boolean") => Ok(ParamKind::Bool),
        _ => Err(format!("unknown parameter kind {v} (use length, int or bool)")),
    }
}

/// Add or update the parameters `list`: fields left out keep their value (new parameters: a
/// length, default required).
fn set_params(tpl: &mut Template, list: &[Value]) -> Result<(), String> {
    for v in list {
        let o = v.as_object().ok_or("each parameter must be an object")?;
        let name = o.get("name").and_then(Value::as_str).ok_or("each parameter needs a 'name'")?;
        let idx = match tpl.params.iter().position(|p| p.name == name) {
            Some(i) => i,
            None => {
                let default =
                    o.get("default").and_then(json_num).ok_or_else(|| format!("new parameter '{name}' needs a default"))?;
                tpl.params.push(Param::new(name, ParamKind::Length, default));
                tpl.params.len() - 1
            }
        };
        let p = &mut tpl.params[idx];
        if let Some(k) = o.get("kind").filter(|k| !k.is_null()) {
            p.kind = kind_arg(k)?;
        }
        if let Some(d) = o.get("default").filter(|d| !d.is_null()) {
            p.default = json_num(d).ok_or_else(|| format!("{name}: default must be a number"))?;
        }
        for (k, slot) in [("min", &mut p.min), ("max", &mut p.max)] {
            match o.get(k) {
                None => {}
                Some(Value::Null) => *slot = None,
                Some(x) => *slot = Some(json_num(x).ok_or_else(|| format!("{name}: {k} must be a number or null"))?),
            }
        }
        if let Some(d) = o.get("description").and_then(Value::as_str) {
            p.description = d.to_string();
        }
    }
    Ok(())
}

/// A number, or a boolean as 1 / 0.
fn json_num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_bool().map(|b| if b { 1.0 } else { 0.0 }))
}

fn shape_from_json(t: &Tech, v: &Value) -> Result<(usize, Geometry), String> {
    let o = v.as_object().ok_or("each shape must be an object")?;
    let layer = layer_index(t, o.get("layer").and_then(Value::as_str).ok_or("each shape needs a 'layer'")?)?;
    let geom = if let Some(r) = o.get("rect") {
        Geometry::Rect(rect_arg(t, r, "rect")?)
    } else if let Some(p) = o.get("polygon") {
        let points = points_arg(t, p, "polygon")?;
        if points.len() < 3 {
            return Err("a polygon needs at least 3 points".into());
        }
        Geometry::Polygon { points }
    } else if let Some(p) = o.get("path") {
        let points = points_arg(t, p, "path")?;
        if points.len() < 2 {
            return Err("a path needs at least 2 points".into());
        }
        let w = dbu(t, o.get("width").and_then(Value::as_f64).ok_or("a path needs a 'width' (µm)")?);
        if w <= 0 {
            return Err("path width is zero after snapping to the grid".into());
        }
        Geometry::Path { points, width: w }
    } else if let Some(text) = o.get("label").and_then(Value::as_str) {
        let at = points_arg(t, &json!([o.get("at").cloned().unwrap_or(Value::Null)]), "label 'at'")?[0];
        let size = dbu(t, o.get("size").and_then(Value::as_f64).unwrap_or(0.2)).max(t.grid.max(1));
        Geometry::Label { text: text.into(), at, size }
    } else {
        return Err("a shape needs one of rect, polygon, path (with width) or label (with at)".into());
    };
    Ok((layer, geom))
}

fn lint_json(tpl: &Template, t: &Tech) -> Vec<Value> {
    tpl.lint(t)
        .into_iter()
        .map(|f| {
            let level = match f.level {
                Level::Error => "error",
                Level::Warning => "warning",
                Level::Info => "info",
            };
            let target = match f.target {
                Target::Template => "template".to_string(),
                Target::Param(i) => format!("param {}", tpl.params.get(i).map_or("?", |p| p.name.as_str())),
                Target::Op(k) => format!("op {k}"),
            };
            json!({"level": level, "target": target, "message": f.message})
        })
        .collect()
}

fn layer_counts(t: &Tech, l: &Layout) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for s in &l.shapes {
        *out.entry(layer_name(t, s.layer)).or_default() += 1;
    }
    out
}

fn shapes_json(t: &Tech, l: &Layout) -> Vec<Value> {
    let pts = |p: &[(i64, i64)]| p.iter().map(|&q| pt_um(t, q)).collect::<Vec<_>>();
    l.shapes
        .iter()
        .take(MAX_LISTED)
        .map(|s| {
            let mut v = json!({"id": s.id, "layer": layer_name(t, s.layer)});
            match &s.geom {
                Geometry::Rect(b) => v["rect"] = rect_um(t, *b),
                Geometry::Polygon { points } => v["polygon"] = json!(pts(points)),
                Geometry::Path { points, width } => {
                    v["path"] = json!(pts(points));
                    v["width"] = json!(um(t, *width));
                }
                Geometry::Label { text, at, .. } => {
                    v["label"] = json!(text);
                    v["at"] = pt_um(t, *at);
                }
            }
            v
        })
        .collect()
}

/// The template as the tools show it (µm, layer names).
fn template_json(t: &Tech, master: &CellRef, tpl: &Template, list_source: bool) -> Value {
    let bbox = tpl.source.shapes.iter().map(|s| s.bbox()).reduce(|a, b| a.union(&b));
    let mut source = json!({
        "bbox_um": bbox.map(|b| rect_um(t, b)),
        "shapes_per_layer": layer_counts(t, &tpl.source),
        "instances": tpl.source.instances.len(),
    });
    if list_source {
        source["shapes"] = json!(shapes_json(t, &tpl.source));
    }
    json!({
        "master": master.to_string(),
        "description": tpl.description,
        "params": tpl.params,
        "ops": tpl.ops.iter().enumerate().map(|(k, o)| op_json(t, k, o)).collect::<Vec<_>>(),
        "source": source,
        "corners": tpl.corners().into_iter().map(|(l, _)| l).collect::<Vec<_>>(),
    })
}

impl State {
    /// Apply the edit arguments shared by create and edit to `tpl`; returns suggestion results.
    fn apply_design_edits(&self, tpl: &mut Template, a: &Args) -> Result<Vec<Value>, String> {
        let p = self.project()?;
        let t = &p.tech;
        if let Some(d) = a.opt_str("description")? {
            tpl.description = d.to_string();
        }
        if let Some(src) = a.opt_str("source")? {
            let r = resolve(p, src)?;
            let l = layout_of(p, &r)?;
            tpl.source = Template::from_layout(&tpl.name, l).source;
        }
        if let Some(ids) = a.get("remove_shapes") {
            let ids: Vec<u64> = ids
                .as_array()
                .and_then(|v| v.iter().map(Value::as_u64).collect())
                .ok_or("remove_shapes must be an array of source shape ids")?;
            for id in ids {
                if !tpl.source.shapes.iter().any(|s| s.id == id) {
                    return Err(format!("the source has no shape {id} (pcell_design_check with list_source lists them)"));
                }
                tpl.source.remove(id);
            }
        }
        if let Some(list) = a.get("add_shapes") {
            for v in list.as_array().ok_or("add_shapes must be an array")? {
                let (layer, geom) = shape_from_json(t, v)?;
                tpl.source.add(layer, geom);
            }
        }
        let mut results = vec![];
        if let Some(list) = a.get("suggest") {
            let asked: Vec<&str> = list
                .as_array()
                .and_then(|v| v.iter().map(Value::as_str).collect())
                .ok_or("suggest must be an array of: contacts, fingers, width, length")?;
            if let Some(bad) = asked.iter().find(|s| !SUGGESTIONS.contains(s)) {
                return Err(format!("unknown suggestion '{bad}' (use contacts, fingers, width, length)"));
            }
            for s in SUGGESTIONS.iter().filter(|s| asked.contains(s)) {
                let r = match *s {
                    "contacts" => match tpl.suggest_contact_fills(t) {
                        0 => Err("No via cuts found".to_string()),
                        n => Ok(format!("{n} contact fill(s)")),
                    },
                    "fingers" => tpl.suggest_fingers(t),
                    "width" => tpl.suggest_width(t),
                    _ => tpl.suggest_length(t),
                };
                results.push(match r {
                    Ok(m) => json!({"suggest": s, "ok": true, "message": m}),
                    Err(m) => json!({"suggest": s, "ok": false, "message": m}),
                });
            }
        }
        if let Some(ks) = a.get("remove_ops") {
            let mut ks: Vec<usize> = ks
                .as_array()
                .and_then(|v| v.iter().map(|x| x.as_u64().map(|k| k as usize)).collect())
                .ok_or("remove_ops must be an array of op indices")?;
            ks.sort_unstable();
            ks.dedup();
            for &k in ks.iter().rev() {
                if k >= tpl.ops.len() {
                    return Err(format!("there is no op {k} ({} ops)", tpl.ops.len()));
                }
                tpl.ops.remove(k);
            }
        }
        if let Some(names) = a.get("remove_params") {
            for n in names.as_array().ok_or("remove_params must be an array of names")? {
                let n = n.as_str().ok_or("remove_params must be names")?;
                let before = tpl.params.len();
                tpl.params.retain(|p| p.name != n);
                if tpl.params.len() == before {
                    return Err(format!("there is no parameter '{n}'"));
                }
            }
        }
        if let Some(list) = a.get("set_params") {
            set_params(tpl, list.as_array().ok_or("set_params must be an array")?)?;
        }
        if let Some(list) = a.get("add_ops") {
            for v in list.as_array().ok_or("add_ops must be an array")? {
                let op = op_from_json(t, v)?;
                match v.get("index").and_then(Value::as_u64) {
                    Some(k) => tpl.ops.insert((k as usize).min(tpl.ops.len()), op),
                    None => tpl.ops.push(op),
                }
            }
        }
        Ok(results)
    }

    /// Store `tpl` as master `lib/<name>` and describe the result.
    fn publish_design(&mut self, lib: &str, tpl: Template, results: Vec<Value>) -> Result<Value, String> {
        let p = self.project_mut()?;
        if p.library_mut(lib).is_none() {
            p.add_library(lib)?;
        }
        let lint = lint_json(&tpl, &p.tech);
        let (master, updated, warnings) =
            p.publish_template(lib, tpl.clone()).map_err(|e| format!("{e}\nlint: {}", Value::Array(lint.clone())))?;
        self.dirty = true;
        let p = self.project()?;
        let mut out = json!({
            "cell": master.to_string(),
            "template": template_json(&p.tech, &master, &tpl, false),
            "lint": lint,
            "variants_updated": updated,
            "warnings": warnings,
        });
        if !results.is_empty() {
            out["suggestions"] = json!(results);
        }
        Ok(out)
    }

    pub(super) fn pcell_design_create(&mut self, a: &Args) -> Result<Value, String> {
        let p = self.project()?;
        let cell = a.str("cell")?;
        let src = a.opt_str("source")?.map(|s| resolve(p, s)).transpose()?;
        let (lib, name) = match cell.split_once('/') {
            Some((l, c)) => (l.to_string(), c.to_string()),
            None => {
                let lib = src
                    .as_ref()
                    .map(|r| r.lib.clone())
                    .filter(|l| l != PCELL_LIB)
                    .or_else(|| p.libraries.iter().map(|l| l.name.clone()).find(|n| n == "work"))
                    .or_else(|| p.libraries.iter().map(|l| l.name.clone()).find(|n| n != PCELL_LIB))
                    .unwrap_or_else(|| "work".into());
                (lib, cell.to_string())
            }
        };
        let r = CellRef::new(&lib, &name);
        if let Some(c) = p.cell(&r) {
            if src.as_ref() == Some(&r) {
                return Err(format!("the master needs a name of its own; e.g. '{lib}/{name}_pc' for the drawing {r}"));
            }
            match &c.pcell {
                Some(PCell::Template { .. }) if !a.opt_bool("replace")?.unwrap_or(false) => {
                    return Err(format!(
                        "{r} already is a PCell Designer master: edit it with pcell_design_edit, or pass replace"
                    ));
                }
                Some(PCell::Template { .. }) => {}
                _ if c.layout.is_some() => return Err(format!("{r} already has a drawn layout; give the PCell another name")),
                _ => {}
            }
        }
        if let Some(s) = &src
            && matches!(p.cell(s).and_then(|c| c.pcell.as_ref()), Some(PCell::Template { .. }))
        {
            return Err(format!("{s} is a PCell Designer master or variant; draw the source in an ordinary cell"));
        }
        let mut tpl = Template::from_layout(&name, &Layout::default());
        let results = self.apply_design_edits(&mut tpl, a)?;
        self.publish_design(&lib, tpl, results)
    }

    pub(super) fn pcell_design_edit(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let mut tpl = template_of(p, &r)?.clone();
        let results = self.apply_design_edits(&mut tpl, a)?;
        self.publish_design(&r.lib, tpl, results)
    }

    pub(super) fn pcell_design_check(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let tpl = template_of(p, &r)?;
        let mut out = template_json(t, &r, tpl, a.opt_bool("list_source")?.unwrap_or(false));
        out["cell"] = json!(r.to_string());
        out["lint"] = json!(lint_json(tpl, t));
        out["placed_variants"] = json!(p.libraries.iter().find(|l| l.name == PCELL_LIB).map_or(0, |l| {
            l.cells.iter().filter(|c| matches!(&c.pcell, Some(PCell::Template { def, .. }) if def.name == tpl.name)).count()
        }));
        let deck = || {
            parse_any(&t.deck_text(), t).map(|d| d.deck).map_err(|errs| {
                let msgs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
                format!("technology rule deck has errors: {}", msgs.join("; "))
            })
        };
        let by_rule = |vs: &[ics_core::drc::Violation]| {
            let mut m: BTreeMap<&str, usize> = BTreeMap::new();
            for v in vs {
                *m.entry(v.rule.as_str()).or_default() += 1;
            }
            json!(m)
        };
        let markers = |vs: &[ics_core::drc::Violation]| {
            vs.iter().take(5).map(|v| json!({"rule": v.rule, "marker_um": rect_um(t, v.marker)})).collect::<Vec<_>>()
        };
        if let Some(vals) = a.get("values") {
            let o = vals.as_object().ok_or("values must be an object of parameter values")?;
            let given: Vec<(String, f64)> = o
                .iter()
                .map(|(k, v)| json_num(v).map(|x| (k.clone(), x)).ok_or_else(|| format!("value of '{k}' must be a number")))
                .collect::<Result<_, _>>()?;
            let lay = tpl.generate(t, &given)?;
            let vs = run_drc_deck(&deck()?, &lay, p, t).violations;
            let bbox = lay.shapes.iter().map(|s| s.bbox()).reduce(|a, b| a.union(&b));
            let mut pv = json!({
                "values": tpl.resolve(&given)?,
                "bbox_um": bbox.map(|b| rect_um(t, b)),
                "shapes_per_layer": layer_counts(t, &lay),
                "drc_violations": vs.len(),
                "by_rule": by_rule(&vs),
                "markers": markers(&vs),
            });
            if a.opt_bool("list_shapes")?.unwrap_or(false) {
                pv["shapes"] = json!(shapes_json(t, &lay));
            }
            out["preview"] = pv;
        }
        if a.opt_bool("corners")?.unwrap_or(true) {
            let runs = tpl.check_corners(t, &deck()?, p);
            let clean = runs.iter().filter(|c| c.result.as_ref().is_ok_and(|c| c.violations.is_empty())).count();
            out["corner_check"] = json!({
                "clean": clean == runs.len(),
                "summary": format!("{clean} of {} corner(s) clean", runs.len()),
                "corners": runs.iter().map(|c| {
                    let mut v = json!({"label": c.label, "values": c.values.iter().cloned().collect::<BTreeMap<_, _>>()});
                    match &c.result {
                        Ok(res) => {
                            v["violations"] = json!(res.violations.len());
                            if !res.violations.is_empty() {
                                v["by_rule"] = by_rule(&res.violations);
                                v["markers"] = json!(markers(&res.violations));
                            }
                        }
                        Err(e) => v["error"] = json!(e),
                    }
                    v
                }).collect::<Vec<_>>(),
            });
        }
        Ok(out)
    }

    pub(super) fn pcell_design_export_skill(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let tpl = template_of(p, &r)?;
        let lib = a.opt_str("lib")?.unwrap_or(&r.lib);
        let (text, warnings) = tpl.to_skill(&p.tech, lib)?;
        match a.opt_str("path")? {
            Some(path) => {
                std::fs::write(path, &text).map_err(|e| format!("{path}: {e}"))?;
                Ok(json!({"cell": r.to_string(), "path": path, "bytes": text.len(), "warnings": warnings}))
            }
            None => Ok(json!({"cell": r.to_string(), "skill": text, "warnings": warnings})),
        }
    }

    /// The designer PCell `master` with parameter values from `params`.
    pub(super) fn template_pcell(&self, master: &str, params: &Map<String, Value>) -> Result<PCell, String> {
        let p = self.project()?;
        let r = resolve(p, master)?;
        let def = template_of(p, &r)?;
        let values: Vec<(String, f64)> = params
            .iter()
            .map(|(k, v)| {
                json_num(v).map(|x| (k.clone(), x)).ok_or_else(|| format!("parameter '{k}' must be a number or boolean"))
            })
            .collect::<Result<_, _>>()?;
        def.resolve(&values)?;
        Ok(PCell::Template { def: Box::new(def.clone()), values })
    }
}

/// The designer masters for project_info.
pub(super) fn designs_json(p: &Project) -> Vec<Value> {
    p.template_masters()
        .into_iter()
        .map(|(r, d)| {
            json!({
                "cell": r.to_string(),
                "params": d.params.iter().map(|q| json!({"name": q.name, "kind": q.kind, "default": q.default})).collect::<Vec<_>>(),
                "ops": d.ops.len(),
            })
        })
        .collect()
}

fn edit_props(t: &mut Map<String, Value>) {
    let expr = "Expression in µm over the parameter names (+ - * / %, comparisons, && || !, parentheses, min, max, abs, floor, ceil, round, sqrt, if(c, a, b)), or a number.";
    let layers = json!({"type": "array", "items": {"type": "string"}, "description": "Layer names the op applies to; empty or left out: all layers (instances move only with all layers)."});
    let rect = json!({"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4, "description": "[x0, y0, x1, y1] µm in source coordinates."});
    let props = json!({
        "description": {"type": "string", "description": "One-line description of the PCell."},
        "source": {"type": "string", "description": "Drawn layout cell whose shapes and instances become the source drawing (replacing the current one; operations are kept)."},
        "add_shapes": {
            "type": "array",
            "description": "Shapes added to the source drawing (µm, snapped to the grid). Each: {layer, rect: [x0, y0, x1, y1]} | {layer, polygon: [[x, y], ...]} | {layer, path: [[x, y], ...], width} | {layer, label: text, at: [x, y], size?}. Label terminals on a routing layer.",
            "items": {"type": "object"},
        },
        "remove_shapes": {"type": "array", "items": {"type": "integer"}, "description": "Source shape ids to delete (pcell_design_check with list_source lists them)."},
        "suggest": {
            "type": "array",
            "items": {"type": "string", "enum": SUGGESTIONS},
            "description": "Read a drawn MOS and add operations, in this order whatever the order given: contacts (via cut clusters become contact fills, so they re-array instead of stretching), fingers (identical equally spaced gates become one unit and a repeat driven by new parameter nf; before any stretch or repeat), width (stretch line through the diffusion driven by new length w), length (stretch line through the first gate driven by new length l). Each reports ok and a message.",
        },
        "remove_ops": {"type": "array", "items": {"type": "integer"}, "description": "Indices of operations to delete (applied before add_ops)."},
        "remove_params": {"type": "array", "items": {"type": "string"}, "description": "Parameters to delete (before set_params)."},
        "set_params": {
            "type": "array",
            "description": "Parameters to add or update by name; fields left out keep their value. kind: length (µm, grid-snapped; default), int, or bool (0 / 1 in expressions). Give min and max so the corner check tests the extremes; null clears one. Booleans may be given as true / false or 1 / 0.",
            "items": {"type": "object", "properties": {
                "name": {"type": "string"}, "kind": {"type": "string", "enum": ["length", "int", "bool"]},
                "default": {"type": "number"}, "min": {"type": "number"}, "max": {"type": "number"},
                "description": {"type": "string"},
            }, "required": ["name"]},
        },
        "add_ops": {
            "type": "array",
            "description": format!("Operations to append (or insert at 'index'). Generation runs conditions, then stretches in list order (each moving later lines and regions), then repeats, then contact fills. Coordinates are µm in the source drawing. stretch {{axis: x (vertical line, moves along x) | y, at: line position, side: high (right / top moves; default) | low | both (centred), amount: expression, layers}}: coordinates beyond the line shift by amount(values) - amount(defaults), so shapes crossing it grow. repeat {{axis, region, count: expression, pitch: expression, at?: where spanning shapes stretch, layers}}: items wholly inside are copied count - 1 times pitch apart; shapes spanning the region follow. condition {{region, when: expression, layers}}: items inside are dropped when when is 0. contact_fill {{rect, via}}: refilled with the largest centred cut array after stretching. {expr}"),
            "items": {"type": "object", "properties": {
                "op": {"type": "string", "enum": ["stretch", "repeat", "condition", "contact_fill"]},
                "axis": {"type": "string", "enum": ["x", "y"]}, "at": {"type": "number"},
                "side": {"type": "string", "enum": ["high", "low", "both"]},
                "amount": {"type": "string"}, "count": {"type": "string"}, "pitch": {"type": "string"},
                "when": {"type": "string"}, "region": rect.clone(), "rect": rect, "via": {"type": "string"},
                "layers": layers, "index": {"type": "integer", "description": "Insert position (default: append)."},
            }, "required": ["op"]},
        },
    });
    t.extend(props.as_object().expect("object").clone());
}

pub(super) fn definitions() -> Vec<Value> {
    let mut create = Map::new();
    create.insert(
        "cell".into(),
        json!({"type": "string", "description": "Master cell to create, 'lib/name' or 'name' (library of the source, else 'work'). Not the source cell itself and not in 'pcells'."}),
    );
    create.insert(
        "replace".into(),
        json!({"type": "boolean", "description": "Start over when the cell already is a designer master (placed variants are regenerated)."}),
    );
    edit_props(&mut create);
    let mut edit = Map::new();
    edit.insert("cell".into(), json!({"type": "string", "description": "The designer master cell."}));
    edit_props(&mut edit);
    vec![
        tool(
            "pcell_design_create",
            &format!(
                "PCell Designer: make a parametric cell (graphical PCell) and store it as master cell 'cell', whose layout is the PCell at its defaults. From a drawn layout: give source (e.g. a hand-drawn or imported device) and usually suggest [contacts, fingers, width, length] for a MOS. From nothing: draw with add_shapes (or draw in a scratch cell with add_rect / add_path / add_label and give it as source), then add parameters (set_params) and operations (add_ops). Accepts every pcell_design_edit field. Returns the template (µm, layer names), lint findings, suggestion results and the parameter corners. Then check with pcell_design_check, refine with pcell_design_edit and place with place_pcell (master). {NOT_SAVED}"
            ),
            Value::Object(create),
            &["cell"],
        ),
        tool(
            "pcell_design_edit",
            &format!(
                "Change a PCell Designer master: description, source drawing (source, add_shapes, remove_shapes), suggestions, parameters (remove_params, set_params) and operations (remove_ops, add_ops), applied in that order. All or nothing: if the PCell would not generate at its defaults nothing changes. The master and every placed variant are regenerated (variants keep their values). Returns the template, lint and suggestion results. {NOT_SAVED}"
            ),
            Value::Object(edit),
            &["cell"],
        ),
        tool(
            "pcell_design_check",
            "Inspect and verify a PCell Designer master without changing it: the template (parameters, operations with indices, source extent; list_source lists the source shapes with ids), lint findings (errors, warnings), placed variant count, the corner check (the technology's DRC deck run on the defaults, each parameter at its min / max, booleans toggled and all at min / max; corners: false skips it) and, with values, a preview of the PCell at those values (bbox, shapes per layer, DRC; list_shapes lists its shapes).",
            json!({
                "cell": {"type": "string", "description": "The designer master cell."},
                "values": {"type": "object", "description": "Parameter values for a preview, e.g. {\"w\": 2, \"nf\": 4}; others take their defaults.", "additionalProperties": {"type": "number"}},
                "corners": {"type": "boolean", "description": "Run the corner check (default true)."},
                "list_source": {"type": "boolean", "description": "List the source drawing's shapes with ids (µm)."},
                "list_shapes": {"type": "boolean", "description": "List the preview's shapes."},
            }),
            &["cell"],
        ),
        tool(
            "pcell_design_export_skill",
            "Export a PCell Designer master to Cadence Virtuoso as a SKILL pcDefinePCell that generates the same layout (the drawing is data; a small engine in the body runs the operations). Returns the SKILL text, or writes it to path. Layer names are the technology's; boolean parameters become 0 / 1 integers.",
            json!({
                "cell": {"type": "string", "description": "The designer master cell."},
                "lib": {"type": "string", "description": "Virtuoso library of the PCell (default: the master's library)."},
                "path": {"type": "string", "description": "File to write (.il); without it the text is returned."},
            }),
            &["cell"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(s: &mut State, name: &str, args: Value) -> Result<Value, String> {
        s.call(name, args.as_object().expect("object")).expect("known tool")
    }

    /// Layout to PCell: a drawn 3-finger NMOS becomes nf / w / l through the suggestions, is
    /// clean at its corners, placed, edited (the placed variant follows) and exported.
    #[test]
    fn from_drawn_layout() {
        let mut p = ics_core::examples::demo_project();
        let drawn = PCell::mos(false, 1000, 180, 3).generate(&p.tech).unwrap();
        p.add_cell("work", Cell { layout: Some(drawn), ..Cell::new("nmos_drawn") }).unwrap();
        let mut s = State { project: Some(p), ..State::default() };
        let same = call(&mut s, "pcell_design_create", json!({"cell": "nmos_drawn", "source": "nmos_drawn"}));
        assert!(same.unwrap_err().contains("name of its own"));

        let made = call(
            &mut s,
            "pcell_design_create",
            json!({"cell": "nmos3", "source": "nmos_drawn", "suggest": ["length", "width", "fingers", "contacts"], "description": "NMOS"}),
        )
        .unwrap();
        assert_eq!(made["cell"], "work/nmos3");
        let names: Vec<&str> =
            made["template"]["params"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["nf", "w", "l"], "{made}");
        assert!(made["suggestions"].as_array().unwrap().iter().all(|r| r["ok"] == true), "{made}");
        assert!(made["lint"].as_array().unwrap().iter().all(|f| f["level"] != "error"), "{made}");
        let again = call(&mut s, "pcell_design_create", json!({"cell": "nmos3", "source": "nmos_drawn"}));
        assert!(again.unwrap_err().contains("pcell_design_edit"));

        let chk = call(&mut s, "pcell_design_check", json!({"cell": "nmos3", "values": {"nf": 4, "w": 2}})).unwrap();
        assert_eq!(chk["corner_check"]["clean"], true, "{}", chk["corner_check"]);
        assert_eq!(chk["preview"]["drc_violations"], 0, "{}", chk["preview"]);
        assert_eq!(chk["placed_variants"], 0);

        let placed = call(
            &mut s,
            "place_pcell",
            json!({"cell": "amp", "master": "nmos3", "params": {"nf": 4, "w": 2}, "x": 0, "y": 0, "name": "M1"}),
        )
        .unwrap();
        assert!(placed["child"].as_str().unwrap().starts_with("pcells/nmos3_"), "{placed}");
        let bad = call(&mut s, "place_pcell", json!({"cell": "amp", "master": "nmos3", "params": {"nf": 2.5}, "x": 0, "y": 5}));
        assert!(bad.unwrap_err().contains("whole number"));
        assert!(call(&mut s, "place_pcell", json!({"cell": "amp", "kind": "nmos", "master": "nmos3", "x": 0, "y": 5})).is_err());

        let fill = made["template"]["ops"].as_array().unwrap().iter().position(|o| o["op"] == "contact_fill").unwrap();
        let ed = call(
            &mut s,
            "pcell_design_edit",
            json!({"cell": "work/nmos3", "set_params": [{"name": "w", "max": 20}], "remove_ops": [], "description": "NMOS, nf / w / l"}),
        )
        .unwrap();
        assert_eq!(ed["variants_updated"], 1, "{ed}");
        let w = ed["template"]["params"].as_array().unwrap().iter().find(|p| p["name"] == "w").unwrap().clone();
        assert_eq!(w["max"], 20.0);
        assert_eq!(w["default"], 1.0, "fields left out keep their value");

        // A broken edit changes nothing.
        let before = call(&mut s, "pcell_design_check", json!({"cell": "nmos3", "corners": false})).unwrap();
        let broken = call(
            &mut s,
            "pcell_design_edit",
            json!({"cell": "nmos3", "add_ops": [{"op": "stretch", "axis": "x", "at": 0, "amount": "zz"}]}),
        );
        assert!(broken.is_err());
        let after = call(&mut s, "pcell_design_check", json!({"cell": "nmos3", "corners": false})).unwrap();
        assert_eq!(before["ops"], after["ops"]);
        assert_eq!(after["ops"][fill]["op"], "contact_fill");

        let sk = call(&mut s, "pcell_design_export_skill", json!({"cell": "nmos3"})).unwrap();
        assert!(sk["skill"].as_str().unwrap().contains("pcDefinePCell"));
        let info = call(&mut s, "project_info", json!({})).unwrap();
        let designs: Vec<&str> = info["pcell_designs"].as_array().unwrap().iter().filter_map(|d| d["cell"].as_str()).collect();
        assert!(designs.contains(&"work/nmos3") && designs.contains(&"work/bayer_fdti_pc"), "{designs:?}");
        assert!(
            call(&mut s, "pcell_design_check", json!({"cell": "nmos_drawn"}))
                .unwrap_err()
                .contains("not a PCell Designer master")
        );
    }

    /// From nothing: shapes, a parameter and a stretch line in one call, then a repeat.
    #[test]
    fn from_scratch() {
        let mut s = State { project: Some(Project::new("empty")), ..State::default() };
        let made = call(
            &mut s,
            "pcell_design_create",
            json!({
                "cell": "plate",
                "add_shapes": [
                    {"layer": "metal1", "rect": [0, 0, 1, 1]},
                    {"layer": "metal1", "label": "A", "at": [0.2, 0.5]},
                ],
                "set_params": [{"name": "w", "default": 1, "min": 0.5, "max": 5}, {"name": "n", "kind": "int", "default": 1, "min": 1, "max": 4}],
                "add_ops": [{"op": "stretch", "axis": "x", "at": 0.6, "amount": "w"}],
            }),
        )
        .unwrap();
        assert_eq!(made["cell"], "work/plate");
        let chk = call(&mut s, "pcell_design_check", json!({"cell": "plate", "values": {"w": 3}, "list_source": true})).unwrap();
        assert_eq!(chk["preview"]["bbox_um"], json!([0.0, 0.0, 3.0, 1.0]), "{chk}");
        assert_eq!(chk["source"]["shapes"].as_array().unwrap().len(), 2);
        assert!(
            chk["lint"].as_array().unwrap().iter().any(|f| f["message"].as_str().unwrap().contains("n is not used")),
            "{chk}"
        );

        call(
            &mut s,
            "pcell_design_edit",
            json!({"cell": "plate", "add_ops": [{"op": "repeat", "axis": "y", "region": [-1, -1, 7, 2], "count": "n", "pitch": 1.5}]}),
        )
        .unwrap();
        let chk = call(&mut s, "pcell_design_check", json!({"cell": "plate", "values": {"n": 3}})).unwrap();
        assert_eq!(chk["preview"]["bbox_um"], json!([0.0, 0.0, 1.0, 4.0]), "{chk}");
        // At w = 0.5 the right edge would cross the line at 0.6: the corner check says so, and
        // moving the line (remove and insert in its place) fixes it.
        let cc = &chk["corner_check"];
        assert_eq!(cc["clean"], false);
        assert!(
            cc["corners"].as_array().unwrap().iter().any(|c| c["error"].as_str().is_some_and(|e| e.contains("too small"))),
            "{cc}"
        );
        let ed = call(
            &mut s,
            "pcell_design_edit",
            json!({"cell": "plate", "remove_ops": [0], "add_ops": [{"op": "stretch", "axis": "x", "at": 0.4, "amount": "w", "index": 0}]}),
        )
        .unwrap();
        assert_eq!(ed["template"]["ops"][0]["at"], 0.4, "{ed}");
        let chk = call(&mut s, "pcell_design_check", json!({"cell": "plate", "values": {"w": 2, "n": 2}})).unwrap();
        assert_eq!(chk["preview"]["bbox_um"], json!([0.0, 0.0, 2.0, 2.5]), "{chk}");
        assert_eq!(chk["corner_check"]["clean"], true, "{}", chk["corner_check"]);
    }
}
