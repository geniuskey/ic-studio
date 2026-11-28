//! Agents and guides: app tools, tutor and Learn checks, the MCP live-mode bridge.

use super::*;

impl IcStudio {
    /// A copilot tool the app answers itself ([`crate::copilot::APP_TOOLS`]).
    pub(super) fn serve_app_tool(&mut self, ctx: &egui::Context, c: Pending) {
        match c.tool.as_str() {
            "ui_state" => {
                let _ = c.reply.send(ToolResult { text: self.ui_state().to_string(), is_error: false });
            }
            "guide_step" => {
                if self.copilot.guide.is_some() {
                    let _ = c.reply.send(ToolResult::error("a step is already waiting for the user; give one step at a time"));
                    return;
                }
                match crate::copilot::parse_guide(&c.arguments) {
                    Err(e) => {
                        let _ = c.reply.send(ToolResult::error(e));
                    }
                    Ok((instruction, why, target, check)) => {
                        self.copilot.log.push(crate::copilot::Entry::Step(instruction.clone()));
                        if let Some(key) = &target {
                            crate::spot::point(ctx, key, &instruction, None);
                        }
                        self.copilot.guide = Some(crate::copilot::Guide {
                            instruction,
                            why,
                            target,
                            check,
                            status: String::new(),
                            passed: false,
                            answer: None,
                            reply: c.reply,
                        });
                    }
                }
            }
            other => {
                let _ = c.reply.send(ToolResult::error(format!("unknown app tool {other}")));
            }
        }
    }

    /// The state of a tutor's step check: passed, and what it says.
    pub(super) fn guide_check(&self, c: &crate::copilot::GuideCheck) -> (bool, String) {
        use crate::copilot::GuideCheck as G;
        use crate::learn::CheckState as S;
        let active = self.active.as_ref().and_then(|r| Some((r, self.project.as_ref()?.cell(r)?)));
        match c {
            G::Design(check) => match learn_check(self.project.as_ref(), &self.sims, &self.lay_eds, check) {
                S::Pass(d) => (true, d),
                S::Fail(d, _) | S::Waiting(d) => (false, d),
            },
            G::Workspace(w) => (self.workspace == *w, tf("Workspace: {}", &[t(self.workspace.name())])),
            G::Open(cell, ws) => {
                let on = active.is_some_and(|(_, c)| c.name == *cell) && ws.is_none_or(|w| w == self.workspace);
                (on, if on { tf("{} is open.", &[cell]) } else { tf("Open {}.", &[cell]) })
            }
            G::Exists(cell) => {
                let there = self.project.as_ref().is_some_and(|p| p.all_cells().any(|(_, c)| c.name == *cell));
                (there, if there { tf("{} exists.", &[cell]) } else { tf("Create the cell {} first.", &[cell]) })
            }
            G::Selected => {
                let any = self.active.as_ref().is_some_and(|r| {
                    self.sch_eds.get(r).is_some_and(|e| !e.selection.is_empty())
                        || self.lay_eds.get(r).is_some_and(|e| !e.selection.is_empty())
                });
                (any, if any { t("Selected.").into() } else { t("Nothing selected yet.").into() })
            }
        }
    }

    /// Follow the tutor's step: point at its target, check it every frame, and answer the
    /// copilot once it passes or the user answers.
    pub(super) fn track_guide(&mut self, ctx: &egui::Context) {
        use crate::copilot::GuideAnswer;
        let Some(g) = &self.copilot.guide else { return };
        if !self.copilot.busy() {
            // The run ended (stopped or failed): the step has nobody to report to.
            self.copilot.guide = None;
            crate::spot::clear(ctx);
            return;
        }
        let check = g.check.clone();
        let (passed, status) = check.as_ref().map_or((false, String::new()), |c| self.guide_check(c));
        let target = g.target.clone();
        if let Some(key) = &target
            && crate::spot::rect(ctx, key).is_none()
            && let Some(w) = crate::spot::anchor(key).and_then(|a| a.workspace)
            && w != self.workspace
        {
            // Not on screen: point at the workspace that shows it.
            crate::spot::point_now(ctx, crate::spot::workspace_key(w));
        }
        let Some(g) = self.copilot.guide.as_mut() else { return };
        g.passed = passed;
        g.status = status.clone();
        let answer = if passed { Some(None) } else { g.answer.take().map(Some) };
        let Some(answer) = answer else {
            ctx.request_repaint_after(Duration::from_millis(300));
            return;
        };
        let g = self.copilot.guide.take().expect("guide");
        crate::spot::clear(ctx);
        let mut out = serde_json::Map::new();
        let ended = match &answer {
            None => "passed",
            Some(GuideAnswer::Done) => "the user pressed Done",
            Some(GuideAnswer::Stuck) => {
                "the user pressed I'm stuck: give a hint (the idea first, then where, then the exact action)"
            }
            Some(GuideAnswer::Said(_)) => "the user wrote a message instead of finishing the step",
        };
        out.insert("ended".into(), ended.into());
        if let Some(GuideAnswer::Said(t)) = &answer {
            out.insert("user_says".into(), t.clone().into());
        }
        if check.is_some() {
            out.insert("check".into(), serde_json::json!({"passed": passed, "detail": status}));
        }
        out.insert("ui_state".into(), self.ui_state());
        if passed {
            self.copilot.log.push(crate::copilot::Entry::Notice(tf("Step done: {}", &[&status])));
        }
        let _ = g.reply.send(ToolResult { text: serde_json::Value::Object(out).to_string(), is_error: false });
    }

    /// The Learn course, in its window or docked in `ui`.
    pub(super) fn learn_ui(&mut self, ctx: &egui::Context, ui: Option<&mut egui::Ui>) -> Vec<Command> {
        let project = self.project.as_ref();
        let find = |name: &str| -> Option<CellRef> { project?.all_cells().find(|(_, c)| c.name == name).map(|(r, _)| r) };
        let check = |c: &crate::learn::Check| learn_check(project, &self.sims, &self.lay_eds, c);
        let cells: Vec<String> = project.map(|p| p.all_cells().map(|(_, c)| c.name.clone()).collect()).unwrap_or_default();
        let env = crate::learn_view::Env { find_cell: &find, check: &check, project_open: project.is_some(), cells: &cells };
        match ui {
            Some(ui) => self.learn.panel(ui, &mut self.settings.learn, &env),
            None => self.learn.window(ctx, &mut self.settings.learn, &env),
        }
    }

    /// What the user sees, for the copilot (`ui_state`).
    pub(super) fn ui_state(&self) -> serde_json::Value {
        use serde_json::json;
        let mut v = serde_json::Map::new();
        v.insert("workspace".into(), self.workspace.name().into());
        v.insert("project_open".into(), self.project.is_some().into());
        if let Some(p) = &self.project {
            v.insert("cells".into(), p.all_cells().map(|(r, _)| r.to_string()).take(80).collect::<Vec<_>>().into());
        }
        if let Some((r, cell)) = self.active.as_ref().and_then(|r| Some((r, self.project.as_ref()?.cell(r)?))) {
            let mut a = serde_json::Map::new();
            a.insert("cell".into(), r.to_string().into());
            if let Some(sch) = &cell.schematic {
                let devices: Vec<String> = sch
                    .instances
                    .iter()
                    .filter(|i| {
                        !matches!(i.kind, ics_core::schematic::DeviceKind::Ground | ics_core::schematic::DeviceKind::Supply)
                    })
                    .map(|i| i.name.clone())
                    .collect();
                a.insert("schematic_devices".into(), devices.into());
                a.insert("wires".into(), sch.wires.len().into());
                if let Some(ed) = self.sch_eds.get(r) {
                    let sel: Vec<String> = ed
                        .selection
                        .iter()
                        .map(|o| match o {
                            ics_core::schematic::SchObj::Instance(id) => {
                                sch.instances.iter().find(|i| i.id == *id).map_or("device".into(), |i| i.name.clone())
                            }
                            ics_core::schematic::SchObj::Wire(_) => "wire".into(),
                            ics_core::schematic::SchObj::Label(_) => "net label".into(),
                        })
                        .collect();
                    a.insert("schematic_selection".into(), sel.into());
                }
            }
            a.insert("has_layout".into(), cell.layout.is_some().into());
            if let Some(sim) = self.sims.get(r) {
                let mut s = serde_json::Map::new();
                s.insert("running".into(), sim.is_running().into());
                if let Some(e) = &sim.error {
                    s.insert("error".into(), e.clone().into());
                }
                if let Some(res) = &sim.results {
                    s.insert("analyses".into(), res.datasets.iter().map(|d| d.analysis.clone()).collect::<Vec<_>>().into());
                    if let Some(op) = res.dataset("op") {
                        let regions: serde_json::Map<String, serde_json::Value> =
                            op.devices.iter().filter_map(|d| Some((d.name.clone(), d.region.clone()?.into()))).collect();
                        s.insert("regions".into(), regions.into());
                    }
                    s.insert("tab_shown".into(), sim.tab.clone().into());
                }
                a.insert("simulation".into(), s.into());
            }
            if let Some(ed) = self.lay_eds.get(r) {
                if let Some(d) = &ed.verify.drc {
                    a.insert("drc_violations".into(), d.violations.len().into());
                }
                if let Some(l) = &ed.verify.lvs {
                    a.insert("lvs".into(), json!({"matched": l.matched, "differences": l.errors.len()}));
                }
            }
            v.insert("active".into(), a.into());
        }
        if self.learn.open {
            v.insert("learn".into(), self.learn_state());
        }
        if self.tutorial.open && !self.tutorial.finished() {
            v.insert("tutorial_step".into(), (self.tutorial.step + 1).into());
        }
        if let Some(g) = &self.copilot.guide {
            v.insert("waiting_step".into(), g.instruction.clone().into());
        }
        v.into()
    }

    /// The Learn chapter open, its checks and the quizzes answered wrong.
    #[doc(hidden)]
    pub fn learn_state(&self) -> serde_json::Value {
        use crate::learn::{Block, CheckState};
        let lang = crate::i18n::current();
        let i = self.learn.chapter;
        let mut checks = vec![];
        for b in crate::learn::blocks(lang, i) {
            if let Block::Check(c) = b {
                let mut c = c.clone();
                if let Some(a) = self.learn.aliases.get(&c.cell) {
                    c.cell = a.clone();
                }
                let (state, detail) = match learn_check(self.project.as_ref(), &self.sims, &self.lay_eds, &c) {
                    CheckState::Pass(d) => ("passed", d),
                    CheckState::Fail(d, _) => ("failed", d),
                    CheckState::Waiting(d) => ("not yet", d),
                };
                checks.push(serde_json::json!({"check": c.label, "cell": c.cell, "state": state, "detail": detail}));
            }
        }
        let prefix = format!("{}#", crate::learn::id(i));
        let wrong: Vec<&String> =
            self.settings.learn.quiz.iter().filter(|(k, ok)| k.starts_with(&prefix) && !**ok).map(|(k, _)| k).collect();
        serde_json::json!({
            "chapter": crate::learn::strip_inline(crate::learn::title(lang, i)),
            "page": if crate::learn::is_project(i) {
                crate::learn::id(i).to_string()
            } else {
                format!("{}learn/{}", if lang == crate::i18n::Lang::Ko { "ko/" } else { "" }, crate::learn::id(i))
            },
            "checks": checks,
            "quizzes_wrong": wrong,
            "chapters_done": self.settings.learn.chapters_done(),
        })
    }

    /// The Learn progress as Markdown, in the UI language: per chapter whether it is done,
    /// the quizzes answered right, and the state of its checks on the user's design now.
    pub fn learn_report(&self) -> String {
        use crate::learn::{Block, CheckState};
        use std::fmt::Write;
        let lang = crate::i18n::current();
        let p = &self.settings.learn;
        let mut out = String::new();
        let _ = writeln!(out, "# {}\n", t("Learn IC Design: progress report"));
        let project = self.project.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| t("no project open").to_string());
        let _ = writeln!(out, "- {}: {}", t("Project"), project);
        let _ = writeln!(
            out,
            "- {}\n",
            tf("{} of {} chapters done", &[&p.chapters_done().to_string(), &crate::learn::count().to_string()])
        );
        let _ = writeln!(out, "| {} | {} | {} | {} |", t("Chapter"), t("Done"), t("Quizzes right"), t("Checks passed"));
        out.push_str("|---|---|---|---|\n");
        let mut details = String::new();
        for i in 0..crate::learn::count() {
            let id = crate::learn::id(i);
            let title = crate::learn::strip_inline(crate::learn::title(lang, i));
            let blocks = crate::learn::blocks(lang, i);
            let quizzes = blocks.iter().filter(|b| matches!(b, Block::Quiz(_))).count();
            let prefix = format!("{id}#");
            let right = p.quiz.iter().filter(|(k, ok)| k.starts_with(&prefix) && **ok).count();
            let (mut checks, mut passed) = (0, 0);
            let mut lines = String::new();
            for b in blocks {
                let Block::Check(check) = b else { continue };
                let mut check = check.clone();
                if let Some(a) = self.learn.aliases.get(&check.cell) {
                    check.cell = a.clone();
                }
                checks += 1;
                let (mark, detail) = match learn_check(self.project.as_ref(), &self.sims, &self.lay_eds, &check) {
                    CheckState::Pass(d) => {
                        passed += 1;
                        ("OK", d)
                    }
                    CheckState::Fail(d, _) => ("×", d),
                    CheckState::Waiting(d) => ("…", d),
                };
                let _ = writeln!(lines, "- {mark} {} (`{}`): {detail}", crate::learn::strip_inline(&check.label), check.cell);
            }
            let done = if p.done.contains(id) { "OK" } else { "" };
            let quiz = if quizzes > 0 { format!("{right} / {quizzes}") } else { "–".into() };
            let check = if checks > 0 { format!("{passed} / {checks}") } else { "–".into() };
            let _ = writeln!(out, "| {title} | {done} | {quiz} | {check} |");
            if !lines.is_empty() {
                let _ = writeln!(details, "\n## {title}\n\n{lines}");
            }
        }
        if !details.is_empty() {
            let _ = writeln!(out, "\n{}", t("Checks on the design, as it is now:"));
            out.push_str(&details);
        }
        out
    }

    /// What the user has done of the tutorial's steps.
    pub(super) fn tutorial_progress(&self) -> crate::tutorial::Progress {
        let inverter = self.project.as_ref().and_then(|p| {
            p.all_cells().find(|(_, c)| c.name == "cmos_inverter" && c.schematic.is_some() && c.layout.is_some()).map(|(r, _)| r)
        });
        let Some(r) = inverter.clone() else {
            return crate::tutorial::Progress {
                workspace: Some(self.workspace),
                palette: self.palette.is_some(),
                ..Default::default()
            };
        };
        let lay = self.lay_eds.get(&r).map(|e| &e.verify);
        crate::tutorial::Progress {
            on_inverter: self.active.as_ref() == Some(&r),
            selected: self.sch_eds.get(&r).is_some_and(|e| !e.selection.is_empty()),
            simulated: self.sims.get(&r).is_some_and(|s| s.results.is_some()),
            calculator: self.calc.open,
            drc: lay.is_some_and(|v| v.drc.is_some()),
            lvs: lay.is_some_and(|v| v.lvs.is_some()),
            derived: self.derive.derived > 0,
            palette: self.palette.is_some(),
            workspace: Some(self.workspace),
            inverter,
        }
    }

    #[doc(hidden)]
    pub fn learn_go_to(&mut self, chapter: usize, anchor: &str) {
        self.learn.go_to(chapter, anchor, &mut self.settings.learn);
    }

    #[doc(hidden)]
    pub fn learn(&self) -> (&crate::learn_view::LearnView, &crate::learn::Progress) {
        (&self.learn, &self.settings.learn)
    }

    #[doc(hidden)]
    pub fn learn_check(&self, c: &crate::learn::Check) -> crate::learn::CheckState {
        learn_check(self.project.as_ref(), &self.sims, &self.lay_eds, c)
    }

    #[doc(hidden)]
    pub fn tutorial(&self) -> &crate::tutorial::Tutorial {
        &self.tutorial
    }

    /// Queue a command, exactly as a menu item or shortcut would. This is the entry point
    /// for scripting / automation front-ends.
    pub fn command(&mut self, c: Command) {
        self.queue.push(c);
    }

    /// Serve MCP tool calls from `ics mcp` (live mode) on this window's project.
    pub fn attach_bridge(&mut self, rx: Receiver<Pending>) {
        self.bridge = Some(Bridge { rx, calls: 0, last: None, waiting: VecDeque::new(), job: None });
        self.info("MCP live mode: tool calls from ics mcp run on the open project".into());
    }

    /// Answer bridge requests in arrival order. Tools in [`tools::BACKGROUND`] run one at a
    /// time on a worker thread against a snapshot; a request behind a waiting one waits too, so
    /// every call sees the effects of the calls before it.
    pub(super) fn serve_bridge(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.bridge else { return };
        b.waiting.extend(b.rx.try_iter());
        let done = match b.job.as_ref().map(|j| j.rx.try_recv()) {
            Some(Ok(out)) => Some(out),
            Some(Err(TryRecvError::Disconnected)) => Some((ToolResult::error("internal error in a background tool call"), None)),
            _ => None,
        };
        if let Some((res, verified)) = done
            && let Some(job) = b.job.take()
        {
            let _ = job.reply.send(res.clone());
            let target = job.cell.filter(|r| self.project.as_ref().is_some_and(|p| p.cell(r).is_some()));
            if let (Some(v), Some(r)) = (verified, &target)
                && let Some(ed) = self.lay_eds.get_mut(r)
            {
                ed.install_verified(v, job.generation);
            }
            self.mcp_done(&job.tool, target, &res, Some(job.started.elapsed()));
        }
        loop {
            let Some(b) = &mut self.bridge else { return };
            let Some(next) = b.waiting.front() else { return };
            let ext = self.extensions.has_tool(&next.tool);
            let background = ext || tools::BACKGROUND.contains(&next.tool.as_str());
            if background && b.job.is_some() {
                return;
            }
            let p = b.waiting.pop_front().expect("front exists");
            if ext {
                self.start_ext_job(ctx, p);
                continue;
            }
            match self.mcp_snapshot(&p.arguments).filter(|_| background) {
                Some((snap, args, cell)) => self.start_mcp_job(ctx, p, snap, args, cell),
                None => {
                    let res = self.mcp_call(&p.tool, &p.arguments);
                    let _ = p.reply.send(res);
                }
            }
        }
    }

    /// The project part a background call needs, with `cell` rewritten to `lib/cell` so it
    /// names the same cell in the snapshot. `None` (run inline) without a project or for an
    /// unknown cell.
    pub(super) fn mcp_snapshot(&self, args: &McpArgs) -> Option<McpSnapshot> {
        let p = self.project.as_ref()?;
        let mut args = args.clone();
        let cell = match args.get("cell").filter(|v| !v.is_null()) {
            Some(v) => Some(tools::find(p, v.as_str()?)?),
            None => None,
        };
        if let Some(r) = &cell {
            args.insert("cell".into(), r.to_string().into());
        }
        Some((tools::snapshot(p, cell.as_ref(), None), args, cell))
    }

    pub(super) fn start_mcp_job(&mut self, ctx: &egui::Context, p: Pending, snap: Project, args: McpArgs, cell: Option<CellRef>) {
        let generation = match &cell {
            Some(r) if matches!(p.tool.as_str(), "run_drc" | "extract" | "run_lvs") => {
                self.lay_eds.entry(r.clone()).or_default().verify.generation()
            }
            _ => 0,
        };
        let (tx, rx) = mpsc::channel();
        let (tool, dirty, ctx) = (p.tool.clone(), self.dirty, ctx.clone());
        std::thread::spawn(move || {
            let mut state = tools::State::in_gui(Some(snap), dirty);
            let res = ics_mcp::call_tool(&mut state, &tool, &args);
            let _ = tx.send((res, state.take_verified()));
            ctx.request_repaint();
        });
        self.info(match &cell {
            Some(r) => format!("MCP · {} {r} running in the background", p.tool),
            None => format!("MCP · {} running in the background", p.tool),
        });
        if let Some(b) = &mut self.bridge {
            b.job = Some(McpJob { tool: p.tool, cell, generation, started: Instant::now(), rx, reply: p.reply });
        }
    }

    /// Run a tool on the UI thread, directly on the open project.
    pub(super) fn mcp_call(&mut self, tool: &str, args: &McpArgs) -> ToolResult {
        let cell = args.get("cell").and_then(|v| v.as_str()).map(str::to_string);
        let replaces = matches!(tool, "open_project" | "new_project");
        if replaces && self.dirty {
            return ToolResult::error(
                "the IC Studio window has unsaved changes: save or discard them there before opening another project",
            );
        }
        // Snapshots of every layout and schematic: whatever the tool changes becomes one undo
        // step per cell, and the editors redraw it.
        let edits = !replaces && !MCP_READ_ONLY.contains(&tool);
        let lay_before = self.project.as_ref().filter(|_| edits).map(layout_snapshot);
        let sch_before = self.project.as_ref().filter(|_| edits).map(schematic_snapshot);
        let mut state = tools::State::in_gui(self.project.take(), self.dirty);
        let res = ics_mcp::call_tool(&mut state, tool, args);
        let (project, dirty) = state.into_parts();
        match project {
            Some(p) if !res.is_error && replaces => {
                let root = p.root.clone();
                self.set_project(p);
                if let Some(root) = root {
                    self.remember_recent(&root);
                }
            }
            p => self.project = p,
        }
        self.dirty = dirty;
        let mut target = cell.as_deref().and_then(|c| self.project.as_ref().and_then(|p| tools::find(p, c)));
        if !res.is_error {
            let mut changed = vec![];
            if let Some(before) = lay_before {
                changed = self.layouts_edited(before);
            }
            if let Some(before) = sch_before {
                self.schematics_edited(before);
            }
            if tool == "run_skill" {
                target = changed.into_iter().next().filter(|_| self.active.is_none());
            }
            if matches!(tool, "copy_cell" | "derive_design")
                && let Some(new) =
                    serde_json::from_str::<serde_json::Value>(&res.text).ok().and_then(|v| v["cell"].as_str().map(str::to_string))
            {
                target = self.project.as_ref().and_then(|p| tools::find(p, &new));
            }
        }
        if !res.is_error
            && matches!(tool, "pcell_design_create" | "pcell_design_edit")
            && let (Some(p), Some(r)) = (&self.project, &target)
        {
            self.pcell_designer.sync(p, r);
        }
        self.mcp_done(tool, target, &res, None);
        res
    }

    /// Log a finished call and show its cell in the matching workspace.
    pub(super) fn mcp_done(&mut self, tool: &str, target: Option<CellRef>, res: &ToolResult, took: Option<Duration>) {
        let summary = mcp_summary(tool, res);
        let took = took.map_or(String::new(), |d| format!(" ({:.1} s)", d.as_secs_f64()));
        let line = match &target {
            Some(r) => format!("MCP · {tool} {r} -> {summary}{took}"),
            None => format!("MCP · {tool} -> {summary}{took}"),
        };
        if res.is_error {
            self.warn(line);
        } else {
            self.info(line);
        }
        if let Some(b) = &mut self.bridge {
            b.calls += 1;
            b.last = Some((Instant::now(), tool.to_string()));
        }
        if let Some(r) = target.filter(|_| !res.is_error) {
            let ws = match tool {
                "simulate" | "set_simulation" => Workspace::Simulation,
                "get_netlist" | "write_schematic" | "copy_cell" | "derive_design" => Workspace::Schematic,
                _ => Workspace::Layout,
            };
            self.queue.push(Command::OpenCell(r.clone(), Some(ws)));
            match tool {
                "simulate" => self.queue.push(Command::RunSimulation),
                "generate_layout" | "flylines" => {
                    let ed = self.lay_eds.entry(r).or_default();
                    ed.show_flylines = true;
                    ed.request_fit();
                }
                _ => {}
            }
        }
    }
}
