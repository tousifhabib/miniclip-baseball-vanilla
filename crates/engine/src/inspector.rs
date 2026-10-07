//! A panel for looking inside the running scene: the tree of objects, what
//! each timeline is doing, and what has just happened.

use std::collections::VecDeque;

use crate::display::{
    ButtonMode, Child, Children, ClipState, Content, Path, bounds_of, child_bounds,
};
use crate::gpu::Stats;
use crate::library::Library;
use crate::math::Matrix;
use crate::stage::Stage;
use bb_format::SymbolInfo;

/// How many recent events the panel keeps.
const LOG_LENGTH: usize = 14;

/// Something the user asked for in the panel, to carry out after it is drawn.
pub enum Action {
    TogglePause,
    Step,
    SetPlaying(Path, bool),
    SetVisible(Path, bool),
    Goto(Path, u16),
}

pub struct Inspector {
    pub open: bool,
    /// The object outlined on the stage.
    selected: Option<Path>,
    log: VecDeque<String>,
}

/// What the panel shows this frame.
pub struct Info<'a> {
    pub stage: &'a Stage,
    pub library: &'a Library,
    pub stats: Stats,
    pub paused: bool,
    pub frames_per_second: f32,
    /// From stage coordinates to window pixels.
    pub base: Matrix,
}

impl Default for Inspector {
    fn default() -> Inspector {
        Inspector::new()
    }
}

impl Inspector {
    pub fn new() -> Inspector {
        Inspector {
            open: false,
            selected: None,
            log: VecDeque::new(),
        }
    }

    /// Adds a line to the list of recent events.
    pub fn note(&mut self, line: String) {
        if self.log.len() == LOG_LENGTH {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    pub fn ui(&mut self, ctx: &egui::Context, info: &Info) -> Vec<Action> {
        let mut actions = Vec::new();
        if !self.open {
            return actions;
        }
        self.outline_selected(ctx, info);

        egui::Window::new("Inspector")
            .default_pos([8.0, 8.0])
            .default_width(300.0)
            .default_height(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let label = if info.paused { "Play" } else { "Pause" };
                    if ui.button(label).clicked() {
                        actions.push(Action::TogglePause);
                    }
                    if ui
                        .add_enabled(info.paused, egui::Button::new("Step"))
                        .clicked()
                    {
                        actions.push(Action::Step);
                    }
                    ui.label(format!("{:.0} fps", info.frames_per_second));
                });
                ui.label(format!(
                    "{} draws, {} blurred layers, {} meshes built",
                    info.stats.draws, info.stats.layers, info.stats.meshes
                ));
                let pointer = &info.stage.pointer;
                ui.label(format!(
                    "pointer at ({:.0}, {:.0}){}",
                    pointer.x,
                    pointer.y,
                    if pointer.on_button() {
                        ", on a button"
                    } else {
                        ""
                    }
                ));
                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::CollapsingHeader::new("Objects")
                            .default_open(true)
                            .show(ui, |ui| {
                                let root = &info.stage.root;
                                self.clip_controls(ui, root, &Path::new(), info, &mut actions);
                                let mut path = Path::new();
                                self.children(
                                    ui,
                                    &root.children,
                                    &mut path,
                                    true,
                                    info,
                                    &mut actions,
                                );
                            });
                        egui::CollapsingHeader::new("Recent events").show(ui, |ui| {
                            if self.log.is_empty() {
                                ui.weak("Nothing yet.");
                            }
                            for line in &self.log {
                                ui.label(line);
                            }
                        });
                    });
            });
        actions
    }

    /// The play switch and frame slider of one clip.
    fn clip_controls(
        &self,
        ui: &mut egui::Ui,
        clip: &ClipState,
        path: &Path,
        info: &Info,
        actions: &mut Vec<Action>,
    ) {
        let count = clip.frame_count(info.library);
        ui.horizontal(|ui| {
            let mut playing = clip.playing;
            if ui.checkbox(&mut playing, "playing").changed() {
                actions.push(Action::SetPlaying(path.clone(), playing));
            }
            if count > 1 {
                let mut frame = clip.frame;
                let slider = egui::Slider::new(&mut frame, 1..=count).text(format!("of {count}"));
                if ui.add(slider).changed() {
                    actions.push(Action::Goto(path.clone(), frame));
                }
            } else {
                ui.weak("one frame");
            }
        });
    }

    /// Lists `children`, topmost first. Objects inside a button cannot be
    /// reached by a path, so they are listed without controls.
    fn children(
        &mut self,
        ui: &mut egui::Ui,
        children: &Children,
        path: &mut Path,
        reachable: bool,
        info: &Info,
        actions: &mut Vec<Action>,
    ) {
        for (&depth, child) in children.iter().rev() {
            path.push(depth);
            let title = title(depth, child, info.library);
            match &child.content {
                Content::Graphic => {
                    ui.horizontal(|ui| self.row(ui, &title, child, path, reachable, actions));
                }
                Content::Clip(clip) => {
                    egui::CollapsingHeader::new(format!("{title}, frame {}", clip.frame))
                        .id_salt(("clip", path.as_slice(), reachable))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                self.row(ui, "this clip", child, path, reachable, actions);
                            });
                            if reachable {
                                self.clip_controls(ui, clip, path, info, actions);
                            }
                            self.children(ui, &clip.children, path, reachable, info, actions);
                        });
                }
                Content::Button(button) => {
                    let mode = match button.mode {
                        ButtonMode::Up => "up",
                        ButtonMode::Over => "over",
                        ButtonMode::Down => "down",
                    };
                    egui::CollapsingHeader::new(format!("{title}, {mode}"))
                        .id_salt(("button", path.as_slice(), reachable))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                self.row(ui, "this button", child, path, reachable, actions);
                            });
                            self.children(ui, button.shown(), path, false, info, actions);
                        });
                }
            }
            path.pop();
        }
    }

    /// One object's line: a switch to hide it, and its name, which selects
    /// it when clicked.
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        child: &Child,
        path: &Path,
        reachable: bool,
        actions: &mut Vec<Action>,
    ) {
        if !reachable {
            ui.label(title);
            return;
        }
        let mut visible = child.visible;
        if ui
            .checkbox(&mut visible, "")
            .on_hover_text("Shown")
            .changed()
        {
            actions.push(Action::SetVisible(path.clone(), visible));
        }
        let selected = self.selected.as_ref() == Some(path);
        if ui.selectable_label(selected, title).clicked() {
            self.selected = (!selected).then(|| path.clone());
        }
    }

    /// Draws a box on the stage around the selected object.
    fn outline_selected(&mut self, ctx: &egui::Context, info: &Info) {
        let Some(path) = &self.selected else {
            return;
        };
        // Walk down to the object, collecting its parents' transforms.
        let mut matrix = info.base;
        let mut children = &info.stage.root.children;
        let mut found = None;
        for (index, depth) in path.iter().enumerate() {
            let Some(child) = children.get(depth) else {
                break;
            };
            if index + 1 == path.len() {
                found = Some(child);
                break;
            }
            let Content::Clip(clip) = &child.content else {
                break;
            };
            matrix = matrix.then_inner(child.matrix);
            children = &clip.children;
        }
        let Some(child) = found else {
            // The timeline has moved on and the object is gone.
            self.selected = None;
            return;
        };
        let bounds = child_bounds(child, matrix, info.library).or_else(|| match &child.content {
            Content::Clip(clip) => bounds_of(
                &clip.children,
                matrix.then_inner(child.matrix),
                info.library,
            ),
            _ => None,
        });
        if let Some([left, top, right, bottom]) = bounds {
            let scale = ctx.pixels_per_point();
            let rect = egui::Rect::from_min_max(
                egui::pos2(left / scale, top / scale),
                egui::pos2(right / scale, bottom / scale),
            );
            ctx.debug_painter().rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 80, 200)),
                egui::StrokeKind::Outside,
            );
        }
    }
}

/// A short description of an object: its depth, what it is, and its name.
fn title(depth: u16, child: &Child, library: &Library) -> String {
    let kind = match library.manifest.symbols.get(&child.symbol).map(|s| &s.info) {
        Some(SymbolInfo::Shape { .. }) => "shape",
        Some(SymbolInfo::MorphShape) => "morph",
        Some(SymbolInfo::Clip { .. }) => "clip",
        Some(SymbolInfo::Button) => "button",
        Some(SymbolInfo::Text) => "text",
        Some(SymbolInfo::EditText) => "text field",
        _ => "object",
    };
    let mut title = format!("{depth}: {kind} {}", child.symbol);
    if let Some(name) = &child.name {
        title.push_str(&format!(" \"{name}\""));
    }
    if child.clip_depth.is_some() {
        title.push_str(" (mask)");
    }
    if !child.filters.is_empty() {
        title.push_str(" (blurred)");
    }
    title
}
