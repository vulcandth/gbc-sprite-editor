use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use gbc_sprite_editor::{
    export,
    model::{Cel, Project, Slice, Target},
    project, source,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const ACCENT: Color32 = Color32::from_rgb(114, 219, 181);
const MUTED: Color32 = Color32::from_rgb(147, 163, 180);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Slice,
    Origin,
    Background,
}

#[derive(Clone)]
enum Pending {
    Open(PathBuf),
    New,
    Close,
}

pub struct Editor {
    project: Project,
    saved: Project,
    project_path: Option<PathBuf>,
    source: Option<image::RgbaImage>,
    texture: Option<egui::TextureHandle>,
    preview: Option<egui::TextureHandle>,
    source_stamp: Option<(SystemTime, u64)>,
    last_poll: Instant,
    hot_reload: bool,
    selected_cel: usize,
    selected_slice: Option<usize>,
    tool: Tool,
    stamp: [u32; 2],
    snap: i32,
    zoom: f32,
    pan: Vec2,
    fit: bool,
    pixel_grid: bool,
    all_cels: bool,
    drag: Option<(Pos2, i32, i32)>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    gesture_base: Option<Project>,
    epoch: u64,
    pending: Option<Pending>,
    close_allowed: bool,
    status: String,
    error: Option<String>,
    show_export: bool,
    show_help: bool,
    object_height: u32,
    tiles_per_row: u32,
    warnings: Vec<String>,
    warnings_dirty: bool,
    hover_pixel: Option<(i32, i32)>,
}

impl Editor {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        Self::configure(&cc.egui_ctx);
        let mut editor = Self::empty();
        if let Some(path) = initial {
            editor.open(&cc.egui_ctx, &path);
        }
        editor
    }

    fn empty() -> Self {
        let project = Project::default();
        Self {
            saved: project.clone(), project, project_path: None, source: None, texture: None, preview: None,
            source_stamp: None, last_poll: Instant::now(), hot_reload: true,
            selected_cel: 0, selected_slice: None, tool: Tool::Slice, stamp: [8, 16], snap: 1,
            zoom: 5.0, pan: Vec2::ZERO, fit: true, pixel_grid: false, all_cels: false, drag: None,
            undo: vec![], redo: vec![], gesture_base: None, epoch: 0, pending: None, close_allowed: false,
            status: "Open a PNG to begin. Your source artwork stays editable in your favorite paint tool.".into(),
            error: None, show_export: false, show_help: false, object_height: 16, tiles_per_row: 16,
            warnings: vec![], warnings_dirty: true, hover_pixel: None,
        }
    }

    fn configure(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(24, 30, 39);
        visuals.window_fill = Color32::from_rgb(29, 36, 46);
        visuals.extreme_bg_color = Color32::from_rgb(16, 21, 28);
        visuals.selection.bg_fill = Color32::from_rgb(43, 90, 77);
        visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        ctx.set_visuals(visuals);
        ctx.global_style_mut(|style| {
            style.spacing.item_spacing = Vec2::new(8.0, 8.0);
            style.spacing.button_padding = Vec2::new(10.0, 6.0);
        });
    }

    fn dirty(&self) -> bool {
        self.project != self.saved
    }
    fn cel(&self) -> &Cel {
        &self.project.cels[self.selected_cel]
    }
    fn cel_mut(&mut self) -> &mut Cel {
        &mut self.project.cels[self.selected_cel]
    }

    fn request(&mut self, ctx: &egui::Context, action: Pending) {
        if self.dirty() {
            self.pending = Some(action);
        } else {
            self.execute(ctx, action);
        }
    }

    fn execute(&mut self, ctx: &egui::Context, action: Pending) {
        match action {
            Pending::Open(path) => self.open(ctx, &path),
            Pending::New => {
                let next_epoch = self.epoch + 1;
                *self = Self::empty();
                self.epoch = next_epoch;
            }
            Pending::Close => {
                self.close_allowed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG or sprite project", &["png", "gbcs", "csv"])
            .pick_file()
        {
            self.request(ctx, Pending::Open(path));
        }
    }

    fn open(&mut self, ctx: &egui::Context, path: &Path) {
        let absolute = match std::path::absolute(path) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let is_png = absolute
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("png"));
        let loaded = if is_png {
            Ok(Project {
                image_path: absolute.to_string_lossy().into_owned(),
                ..Project::default()
            })
        } else {
            project::load(&absolute)
        };
        match loaded {
            Ok(loaded) => {
                // A missing PNG does not prevent recovering the slice definitions.
                let decoded = source::load_png(Path::new(&loaded.image_path));
                if is_png && let Err(error) = &decoded {
                    self.error = Some(error.clone());
                    return;
                }
                self.project = loaded;
                self.saved = self.project.clone();
                self.project_path = if is_png { None } else { Some(absolute.clone()) };
                self.source = None;
                self.texture = None;
                self.preview = None;
                self.selected_cel = 0;
                self.selected_slice = (!self.cel().slices.is_empty()).then_some(0);
                self.tool = if self.selected_slice.is_some() {
                    Tool::Select
                } else {
                    Tool::Slice
                };
                self.drag = None;
                self.undo.clear();
                self.redo.clear();
                self.gesture_base = None;
                self.epoch += 1;
                self.fit = true;
                self.warnings_dirty = true;
                match decoded {
                    _ if self.project.image_path.is_empty() => {
                        self.source_stamp = None;
                    }
                    Ok(pixels) => self.install_source(ctx, pixels),
                    Err(e) => {
                        self.error = Some(format!(
                            "{e}\nUse Relink PNG to locate the source; your slices are loaded."
                        ))
                    }
                }
                self.status = format!("Opened {}", absolute.display());
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn install_source(&mut self, ctx: &egui::Context, pixels: image::RgbaImage) {
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [pixels.width() as usize, pixels.height() as usize],
            pixels.as_raw(),
        );
        self.texture = Some(ctx.load_texture("source", color, egui::TextureOptions::NEAREST));
        self.source = Some(pixels);
        self.source_stamp = file_stamp(Path::new(&self.project.image_path));
        self.warnings_dirty = true;
    }

    fn reload(&mut self, ctx: &egui::Context, manual: bool) {
        if self.project.image_path.is_empty() {
            return;
        }
        match source::load_png(Path::new(&self.project.image_path)) {
            Ok(pixels) => {
                self.install_source(ctx, pixels);
                self.status = "PNG reloaded. Slice positions and palettes preserved.".into();
            }
            Err(e) => {
                if manual {
                    self.error = Some(e);
                } else {
                    self.status = format!("Waiting for a readable PNG: {e}");
                }
                // Keep the old image and timestamp, so interrupted/atomic saves are retried.
            }
        }
    }

    fn save(&mut self, save_as: bool) -> bool {
        let path = if save_as || self.project_path.is_none() {
            let filename = self
                .project_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("sprites.gbcs");
            rfd::FileDialog::new()
                .add_filter("Sprite project (CSV)", &["gbcs"])
                .set_file_name(filename)
                .save_file()
        } else {
            self.project_path.clone()
        };
        let Some(mut path) = path else {
            return false;
        };
        if path.extension().is_none() {
            path.set_extension("gbcs");
        }
        if same_path(&path, Path::new(&self.project.image_path)) {
            self.error = Some("Choose a project filename different from the source PNG.".into());
            return false;
        }
        match project::save(&self.project, &path) {
            Ok(()) => {
                self.saved = self.project.clone();
                self.status = format!("Saved {}", path.display());
                self.project_path = Some(path);
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    fn relink(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Source PNG", &["png"])
            .pick_file()
        {
            match source::load_png(&path) {
                Ok(pixels) => {
                    self.project.image_path = path.to_string_lossy().into_owned();
                    self.install_source(ctx, pixels);
                    self.status = "Source PNG relinked; slices kept.".into();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    fn duplicate(&mut self) {
        if let Some(index) = self.selected_slice {
            let mut slice = self.cel().slices[index].clone();
            slice.name = format!("{}_copy", slice.name.chars().take(251).collect::<String>());
            self.cel_mut().slices.push(slice);
            self.selected_slice = Some(self.cel().slices.len() - 1);
        }
    }

    fn delete_slice(&mut self) {
        if let Some(index) = self.selected_slice.take() {
            self.cel_mut().slices.remove(index);
        }
    }

    fn undo(&mut self, redo: bool) {
        // Finish any current edit before traversing history.
        self.finish_gesture();
        let snapshot = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(snapshot) = snapshot {
            let old_path = self.project.image_path.clone();
            let previous = std::mem::replace(&mut self.project, snapshot);
            if redo {
                self.undo.push(previous);
            } else {
                self.redo.push(previous);
            }
            self.selected_cel = self.selected_cel.min(self.project.cels.len() - 1);
            self.selected_slice = None;
            self.drag = None;
            self.warnings_dirty = true;
            self.epoch += 1;
            if old_path != self.project.image_path {
                self.source_stamp = None;
                self.source = None;
                self.texture = None;
            }
        }
    }

    fn push_undo(&mut self, project: Project) {
        if self.undo.last() != Some(&project) {
            self.undo.push(project);
        }
        if self.undo.len() > 128 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn finish_gesture(&mut self) {
        if let Some(before) = self.gesture_base.take()
            && before != self.project
        {
            self.push_undo(before);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() || self.error.is_some() || self.show_export || self.show_help {
            return;
        }
        let command = ctx.input(|i| i.modifiers.command);
        let shift = ctx.input(|i| i.modifiers.shift);
        if command && ctx.input(|i| i.key_pressed(egui::Key::S)) {
            self.save(shift);
        }
        if command && ctx.input(|i| i.key_pressed(egui::Key::O)) {
            self.open_dialog(ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if command {
            if ctx.input(|i| i.key_pressed(egui::Key::Z)) {
                self.undo(shift);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Y)) {
                self.undo(true);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::D)) {
                self.duplicate();
            }
        } else {
            if ctx.input(|i| i.key_pressed(egui::Key::V)) {
                self.tool = Tool::Select;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::B)) {
                self.tool = Tool::Slice;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::O)) {
                self.tool = Tool::Origin;
            }
            if ctx
                .input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
            {
                self.delete_slice();
            }
            if let Some(index) = self.selected_slice {
                let dx = ctx.input(|i| {
                    i.key_pressed(egui::Key::ArrowRight) as i32
                        - i.key_pressed(egui::Key::ArrowLeft) as i32
                });
                let dy = ctx.input(|i| {
                    i.key_pressed(egui::Key::ArrowDown) as i32
                        - i.key_pressed(egui::Key::ArrowUp) as i32
                });
                let step = if shift { 8 } else { 1 };
                let slice = &mut self.cel_mut().slices[index];
                slice.x = (slice.x + dx * step).clamp(-1_000_000, 1_000_000);
                slice.y = (slice.y + dy * step).clamp(-1_000_000, 1_000_000);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("GBC").strong().size(22.0).color(ACCENT));
            ui.label(RichText::new("SPRITE EDITOR").strong().size(15.0));
            ui.separator();
            if ui.button("Open…").clicked() {
                self.open_dialog(&ctx);
            }
            if ui.button("Save").clicked() {
                self.save(false);
            }
            if ui.button("Save as…").clicked() {
                self.save(true);
            }
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                self.undo(false);
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                .clicked()
            {
                self.undo(true);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Help").clicked() {
                    self.show_help = true;
                }
                if ui
                    .add_enabled(
                        self.source.is_some(),
                        egui::Button::new(RichText::new("Export PNG + CSV").color(ACCENT)),
                    )
                    .clicked()
                {
                    self.show_export = true;
                }
            });
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tool, Tool::Select, "Select  V");
            ui.selectable_value(&mut self.tool, Tool::Slice, "Place slice  B");
            ui.selectable_value(&mut self.tool, Tool::Origin, "Set origin  O");
            ui.separator();
            if ui.selectable_label(self.stamp == [8, 8], "8 × 8").clicked() {
                self.stamp = [8, 8];
                self.tool = Tool::Slice;
            }
            if ui
                .selectable_label(self.stamp == [8, 16], "8 × 16")
                .clicked()
            {
                self.stamp = [8, 16];
                self.tool = Tool::Slice;
            }
            ui.add(
                egui::DragValue::new(&mut self.stamp[0])
                    .range(1..=256)
                    .prefix("W "),
            );
            ui.add(
                egui::DragValue::new(&mut self.stamp[1])
                    .range(1..=256)
                    .prefix("H "),
            );
            ui.separator();
            egui::ComboBox::from_id_salt("snap")
                .selected_text(format!("Snap {} px", self.snap))
                .width(90.0)
                .show_ui(ui, |ui| {
                    for snap in [1, 2, 4, 8] {
                        ui.selectable_value(&mut self.snap, snap, format!("{snap} px"));
                    }
                });
            ui.checkbox(&mut self.pixel_grid, "Pixel grid");
        });
    }

    fn project_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical()
            .id_salt("project-scroll")
            .show(ui, |ui| {
                section(ui, "PROJECT");
                ui.label(
                    RichText::new(
                        self.project_path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Untitled project".into()),
                    )
                    .strong(),
                );
                ui.label(
                    RichText::new(if self.dirty() {
                        "● Unsaved changes"
                    } else {
                        "All changes saved"
                    })
                    .small()
                    .color(if self.dirty() { ACCENT } else { MUTED }),
                );
                if ui.small_button("New project").clicked() {
                    self.request(&ctx, Pending::New);
                }
                ui.add_space(8.0);
                section(ui, "SOURCE PNG");
                let source_name = Path::new(&self.project.image_path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "No image loaded".into());
                ui.label(source_name)
                    .on_hover_text(&self.project.image_path);
                if let Some(image) = &self.source {
                    ui.label(
                        RichText::new(format!("{} × {} pixels", image.width(), image.height()))
                            .color(MUTED),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.small_button("Relink PNG…").clicked() {
                        self.relink(&ctx);
                    }
                    if ui.small_button("Reload").clicked() {
                        self.reload(&ctx, true);
                    }
                });
                ui.checkbox(&mut self.hot_reload, "Hot reload PNG");
                ui.horizontal(|ui| {
                    ui.label("Target");
                    ui.selectable_value(&mut self.project.target, Target::Dmg, "GB");
                    ui.selectable_value(&mut self.project.target, Target::Cgb, "GBC");
                });
                ui.add_space(8.0);
                section(ui, "TRANSPARENT BACKGROUND");
                ui.label(
                    RichText::new("PNG alpha is always preserved.")
                        .small()
                        .color(MUTED),
                );
                ui.horizontal(|ui| {
                    if let Some(rgb) = self.project.transparent_color {
                        swatch(ui, rgb, true);
                    }
                    if ui
                        .selectable_label(self.tool == Tool::Background, "Pick color")
                        .clicked()
                    {
                        self.tool = Tool::Background;
                    }
                    if ui.small_button("Clear").clicked() {
                        self.project.transparent_color = None;
                    }
                });
                ui.add_space(8.0);
                section(ui, "CELS");
                let mut select = None;
                for (i, cel) in self.project.cels.iter().enumerate() {
                    if ui
                        .selectable_label(
                            i == self.selected_cel,
                            format!("{}   · {}", cel.name, cel.slices.len()),
                        )
                        .clicked()
                    {
                        select = Some(i);
                    }
                }
                if let Some(i) = select {
                    self.selected_cel = i;
                    self.selected_slice = None;
                }
                ui.horizontal(|ui| {
                    if ui.button("+ Cel").clicked() {
                        let n = self.project.cels.len();
                        self.project.cels.push(Cel {
                            name: format!("cel_{n}"),
                            origin_x: 0,
                            origin_y: 0,
                            slices: vec![],
                        });
                        self.selected_cel = n;
                        self.selected_slice = None;
                    }
                    if ui
                        .add_enabled(self.project.cels.len() > 1, egui::Button::new("− Cel"))
                        .on_hover_text("Remove this cel and its slices; Undo restores it.")
                        .clicked()
                    {
                        self.project.cels.remove(self.selected_cel);
                        self.selected_cel = self.selected_cel.min(self.project.cels.len() - 1);
                        self.selected_slice = None;
                    }
                });
                ui.checkbox(&mut self.all_cels, "Show other cels");
                ui.add_space(8.0);
                section(ui, "SLICES · DRAW ORDER");
                let mut select = None;
                for (i, slice) in self.cel().slices.iter().enumerate() {
                    if ui
                        .selectable_label(
                            self.selected_slice == Some(i),
                            format!(
                                "{:02}  {}   {}×{}",
                                i, slice.name, slice.width, slice.height
                            ),
                        )
                        .clicked()
                    {
                        select = Some(i);
                    }
                }
                if let Some(i) = select {
                    self.selected_slice = Some(i);
                    self.tool = Tool::Select;
                }
                if self.cel().slices.is_empty() {
                    ui.label(
                        RichText::new("Choose Place slice, then click the canvas.").color(MUTED),
                    );
                }
            });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("inspector-scroll").show(ui, |ui| {
            section(ui, "CEL ORIGIN");
            ui.add(egui::TextEdit::singleline(&mut self.cel_mut().name).char_limit(256));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.cel_mut().origin_x).range(-1_000_000..=1_000_000).prefix("X "));
                ui.add(egui::DragValue::new(&mut self.cel_mut().origin_y).range(-1_000_000..=1_000_000).prefix("Y "));
            });
            ui.label(RichText::new("Exported positions are relative to this crosshair.").small().color(MUTED));
            ui.separator();
            section(ui, "SLICE INSPECTOR");
            if let Some(index) = self.selected_slice {
                let origin = [self.cel().origin_x, self.cel().origin_y];
                let slice = &mut self.cel_mut().slices[index];
                ui.add(egui::TextEdit::singleline(&mut slice.name).char_limit(256));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut slice.x).range(-1_000_000..=1_000_000).prefix("X "));
                    ui.add(egui::DragValue::new(&mut slice.y).range(-1_000_000..=1_000_000).prefix("Y "));
                });
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut slice.width).range(1..=256).prefix("W "));
                    ui.add(egui::DragValue::new(&mut slice.height).range(1..=256).prefix("H "));
                });
                ui.label(RichText::new(format!("Relative: {}, {}", slice.x-origin[0], slice.y-origin[1])).color(MUTED));
                ui.add(egui::DragValue::new(&mut slice.palette).range(0..=7).prefix("OBJ palette ")).on_hover_text("GB uses 0–1; GBC uses 0–7. Palette IDs are metadata, not color quantization.");
                ui.horizontal(|ui| {
                    if ui.button("Duplicate").clicked() { self.duplicate(); }
                    if ui.button("Delete").clicked() { self.delete_slice(); }
                });
                if self.selected_slice != Some(index) { return; }
                ui.horizontal(|ui| {
                    if ui.add_enabled(index > 0, egui::Button::new("Move up")).clicked() { self.cel_mut().slices.swap(index,index-1); self.selected_slice = Some(index-1); }
                    if ui.add_enabled(index+1 < self.cel().slices.len(), egui::Button::new("Move down")).clicked() { self.cel_mut().slices.swap(index,index+1); self.selected_slice = Some(index+1); }
                });
                let index = self.selected_slice.unwrap();
                ui.add_space(8.0);
                section(ui, "COLOR FILTER");
                let colors: Vec<[u8;3]> = self.source.as_ref().map(|image| source_colors(image, &self.cel().slices[index], self.project.transparent_color)).unwrap_or_default();
                let mut filtering = self.cel().slices[index].colors.is_some();
                if ui.checkbox(&mut filtering, "Keep only selected colors").changed() {
                    self.cel_mut().slices[index].colors = if filtering { Some(colors.iter().take(256).copied().collect()) } else { None };
                }
                ui.label(RichText::new("Click swatches to keep/remove colors. Overlap slices for separate OBJ palettes.").small().color(MUTED));
                let mut displayed = colors;
                if let Some(mask) = &self.cel().slices[index].colors { for color in mask { if !displayed.contains(color) { displayed.push(*color); } } }
                ui.horizontal_wrapped(|ui| {
                    for rgb in displayed.iter().take(64) {
                        let active = self.cel().slices[index].colors.as_ref().is_none_or(|mask| mask.contains(rgb));
                        if swatch(ui,*rgb,active).clicked() {
                            let mask = self.cel_mut().slices[index].colors.get_or_insert_with(|| displayed.iter().take(256).copied().collect());
                            if let Some(i) = mask.iter().position(|c| c == rgb) { mask.remove(i); } else if mask.len() < 256 { mask.push(*rgb); }
                        }
                    }
                });
                if displayed.len() > 64 { ui.label("Showing first 64 colors; use pixel art with a small palette."); }
                ui.horizontal(|ui| {
                    if ui.small_button("All colors").clicked() { self.cel_mut().slices[index].colors = None; }
                    if ui.small_button("No colors").clicked() { self.cel_mut().slices[index].colors = Some(vec![]); }
                });
                if let Some(source) = &self.source {
                    let pixels = export::slice_image(source, &self.cel().slices[index], self.project.transparent_color);
                    let size = [pixels.width() as usize, pixels.height() as usize];
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw());
                    if let Some(texture) = &mut self.preview { texture.set(color, egui::TextureOptions::NEAREST); }
                    else { self.preview = Some(ui.ctx().load_texture("slice-preview", color, egui::TextureOptions::NEAREST)); }
                    let scale = (180.0 / size[0].max(size[1]) as f32).clamp(1.0,8.0);
                    let (rect,_) = ui.allocate_exact_size(Vec2::new(size[0] as f32,size[1] as f32)*scale,Sense::hover());
                    checker(ui.painter(),rect,8.0);
                    ui.painter().image(self.preview.as_ref().unwrap().id(),rect,Rect::from_min_max(Pos2::ZERO,Pos2::new(1.0,1.0)),Color32::WHITE);
                    ui.label(RichText::new("Filtered export preview").small().color(MUTED));
                }
            } else { ui.label(RichText::new("Select a slice to edit its position, size, and colors.").color(MUTED)); }
            ui.separator();
            section(ui, "HARDWARE CHECK");
            ui.horizontal(|ui| {
                ui.label("OBJ size");
                if ui.selectable_value(&mut self.object_height,8,"8×8").changed() { self.warnings_dirty = true; }
                if ui.selectable_value(&mut self.object_height,16,"8×16").changed() { self.warnings_dirty = true; }
            });
            if self.warnings.is_empty() { ui.label(RichText::new("No warnings for current settings.").color(ACCENT)); }
            for warning in self.warnings.iter().take(30) { ui.label(RichText::new(format!("• {warning}")).small().color(Color32::from_rgb(239,191,112))); }
            if self.warnings.len()>30 { ui.label(format!("{} more warnings in export report",self.warnings.len()-30)); }
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("SOURCE CANVAS").small().strong().color(MUTED));
            ui.separator();
            ui.label(format!(
                "{}  /  {} slices",
                self.cel().name,
                self.cel().slices.len()
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Fit").clicked() {
                    self.fit = true;
                }
                ui.add(
                    egui::Slider::new(&mut self.zoom, 0.5..=32.0)
                        .logarithmic(true)
                        .suffix("×")
                        .max_decimals(1),
                );
            });
        });
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(13, 18, 24));
        let Some(texture) = &self.texture else {
            painter.text(
                rect.center() - Vec2::new(0.0, 36.0),
                egui::Align2::CENTER_CENTER,
                "Every pixel, in its place.",
                egui::FontId::proportional(26.0),
                Color32::WHITE,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Open or drop a PNG to start slicing.",
                egui::FontId::proportional(16.0),
                MUTED,
            );
            painter.text(
                rect.center() + Vec2::new(0.0, 32.0),
                egui::Align2::CENTER_CENTER,
                "8×8 · 8×16 · custom sizes · overlapping palettes",
                egui::FontId::proportional(14.0),
                ACCENT,
            );
            return;
        };
        let image_size = texture.size_vec2();
        if self.fit {
            self.zoom = ((rect.width() - 64.0) / image_size.x)
                .min((rect.height() - 64.0) / image_size.y)
                .clamp(0.5, 16.0);
            self.pan = (rect.size() - image_size * self.zoom) * 0.5;
            self.fit = false;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            if ui.input(|i| i.modifiers.command) {
                if scroll.y != 0.0 {
                    let old_zoom = self.zoom;
                    self.zoom = (self.zoom * (scroll.y * 0.004).exp()).clamp(0.5, 32.0);
                    if let Some(pointer) = response.hover_pos() {
                        self.pan = pointer
                            - rect.min
                            - (pointer - rect.min - self.pan) * (self.zoom / old_zoom);
                    }
                }
            } else {
                self.pan += scroll;
            }
        }
        if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
        {
            self.pan += ui.input(|i| i.pointer.delta());
        }
        let image_rect = Rect::from_min_size(rect.min + self.pan, image_size * self.zoom);
        checker(
            &painter,
            image_rect.intersect(rect),
            8.0 * self.zoom.max(1.0),
        );
        painter.image(
            texture.id(),
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            image_rect,
            0.0,
            Stroke::new(1.0_f32, Color32::from_gray(80)),
            egui::StrokeKind::Outside,
        );
        if self.pixel_grid && self.zoom >= 5.0 {
            let min = ((rect.min - image_rect.min) / self.zoom).max(Vec2::ZERO);
            let max = ((rect.max - image_rect.min) / self.zoom).min(image_size);
            for x in min.x.floor() as u32..=max.x.ceil() as u32 {
                let px = image_rect.min.x + x as f32 * self.zoom;
                painter.line_segment(
                    [
                        Pos2::new(px, image_rect.min.y),
                        Pos2::new(px, image_rect.max.y),
                    ],
                    Stroke::new(0.5_f32, Color32::from_black_alpha(60)),
                );
            }
            for y in min.y.floor() as u32..=max.y.ceil() as u32 {
                let py = image_rect.min.y + y as f32 * self.zoom;
                painter.line_segment(
                    [
                        Pos2::new(image_rect.min.x, py),
                        Pos2::new(image_rect.max.x, py),
                    ],
                    Stroke::new(0.5_f32, Color32::from_black_alpha(60)),
                );
            }
        }
        self.hover_pixel = response
            .hover_pos()
            .map(|pos| pixel_at(pos, image_rect.min, self.zoom));
        let pointer = response.interact_pointer_pos();
        let primary_start = response.drag_started_by(egui::PointerButton::Primary);
        let primary_click = response.clicked_by(egui::PointerButton::Primary);
        if self.tool == Tool::Select
            && (primary_start || primary_click)
            && let Some(pos) = pointer
        {
            let hit_pos = if primary_start {
                ui.input(|input| input.pointer.press_origin())
                    .unwrap_or(pos)
            } else {
                pos
            };
            let (x, y) = pixel_at(hit_pos, image_rect.min, self.zoom);
            // Clicking the selected slice preserves it even under overlapping slices.
            let selected_hit = self
                .selected_slice
                .filter(|i| self.cel().slices[*i].contains(x, y));
            self.selected_slice = selected_hit.or_else(|| {
                self.cel()
                    .slices
                    .iter()
                    .rposition(|slice| slice.contains(x, y))
            });
            if primary_start && let Some(i) = self.selected_slice {
                let start = ui
                    .input(|input| input.pointer.press_origin())
                    .unwrap_or(pos);
                self.drag = Some((start, self.cel().slices[i].x, self.cel().slices[i].y));
            }
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && self.tool == Tool::Select
            && let (Some((start, x, y)), Some(pos), Some(i)) =
                (self.drag, pointer, self.selected_slice)
        {
            let delta = (pos - start) / self.zoom;
            let snap = self.snap;
            let slice = &mut self.cel_mut().slices[i];
            slice.x = snapped(x + delta.x.round() as i32, snap).clamp(-1_000_000, 1_000_000);
            slice.y = snapped(y + delta.y.round() as i32, snap).clamp(-1_000_000, 1_000_000);
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        if primary_click && let Some(pos) = pointer {
            let (x, y) = pixel_at(pos, image_rect.min, self.zoom);
            match self.tool {
                Tool::Slice => {
                    let slice = Slice {
                        name: format!("slice_{}", self.cel().slices.len()),
                        x: snapped(x, self.snap).clamp(-1_000_000, 1_000_000),
                        y: snapped(y, self.snap).clamp(-1_000_000, 1_000_000),
                        width: self.stamp[0],
                        height: self.stamp[1],
                        ..Slice::default()
                    };
                    self.cel_mut().slices.push(slice);
                    self.selected_slice = Some(self.cel().slices.len() - 1);
                }
                Tool::Origin => {
                    self.cel_mut().origin_x = x.clamp(-1_000_000, 1_000_000);
                    self.cel_mut().origin_y = y.clamp(-1_000_000, 1_000_000);
                }
                Tool::Background => {
                    if let Some(source) = &self.source
                        && x >= 0
                        && y >= 0
                        && (x as u32) < source.width()
                        && (y as u32) < source.height()
                    {
                        let rgba = source.get_pixel(x as u32, y as u32).0;
                        self.project.transparent_color = Some([rgba[0], rgba[1], rgba[2]]);
                        self.tool = Tool::Select;
                    }
                }
                Tool::Select => {}
            }
        }
        for (cel_index, cel) in self.project.cels.iter().enumerate() {
            if cel_index != self.selected_cel && !self.all_cels {
                continue;
            }
            let selected_index = if cel_index == self.selected_cel {
                self.selected_slice
            } else {
                None
            };
            for i in (0..cel.slices.len())
                .filter(|i| Some(*i) != selected_index)
                .chain(selected_index)
            {
                let slice = &cel.slices[i];
                let selected = cel_index == self.selected_cel && self.selected_slice == Some(i);
                let color = if selected {
                    ACCENT
                } else if cel_index == self.selected_cel {
                    Color32::from_rgb(245, 186, 104)
                } else {
                    Color32::from_gray(100)
                };
                let sr = Rect::from_min_size(
                    image_rect.min + Vec2::new(slice.x as f32, slice.y as f32) * self.zoom,
                    Vec2::new(slice.width as f32, slice.height as f32) * self.zoom,
                );
                if !sr.intersects(rect) {
                    continue;
                }
                painter.rect_filled(
                    sr,
                    0.0,
                    color.gamma_multiply(if selected { 0.12 } else { 0.035 }),
                );
                painter.rect_stroke(
                    sr,
                    0.0,
                    Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, color),
                    egui::StrokeKind::Inside,
                );
                if self.zoom >= 2.0 {
                    let badge = Rect::from_min_size(sr.min, Vec2::new(19.0, 15.0));
                    painter.rect_filled(badge, 0.0, Color32::from_black_alpha(190));
                    painter.text(
                        badge.center(),
                        egui::Align2::CENTER_CENTER,
                        i.to_string(),
                        egui::FontId::monospace(10.0),
                        color,
                    );
                }
            }
        }
        let origin = image_rect.min
            + Vec2::new(self.cel().origin_x as f32, self.cel().origin_y as f32) * self.zoom;
        painter.line_segment(
            [origin - Vec2::new(11.0, 0.0), origin + Vec2::new(11.0, 0.0)],
            Stroke::new(2.0_f32, Color32::from_rgb(255, 113, 136)),
        );
        painter.line_segment(
            [origin - Vec2::new(0.0, 11.0), origin + Vec2::new(0.0, 11.0)],
            Stroke::new(2.0_f32, Color32::from_rgb(255, 113, 136)),
        );
        painter.circle_stroke(origin, 4.0, Stroke::new(1.0_f32, Color32::WHITE));
        if self.tool == Tool::Slice
            && let Some((x, y)) = self.hover_pixel
        {
            let preview = Rect::from_min_size(
                image_rect.min
                    + Vec2::new(snapped(x, self.snap) as f32, snapped(y, self.snap) as f32)
                        * self.zoom,
                Vec2::new(self.stamp[0] as f32, self.stamp[1] as f32) * self.zoom,
            );
            painter.rect_stroke(
                preview,
                0.0,
                Stroke::new(2.0_f32, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
            response.on_hover_cursor(egui::CursorIcon::Crosshair);
        } else if matches!(self.tool, Tool::Origin | Tool::Background) {
            response.on_hover_cursor(egui::CursorIcon::Crosshair);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading("Save your slice changes?");
                ui.label("This project has unsaved changes.");
                ui.horizontal(|ui| {
                    if ui.button("Save and continue").clicked()
                        && self.save(false)
                        && let Some(action) = self.pending.take()
                    {
                        self.execute(ctx, action);
                    }
                    if ui.button("Discard changes").clicked()
                        && let Some(action) = self.pending.take()
                    {
                        self.execute(ctx, action);
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending = None;
                    }
                });
            });
        }
        if let Some(message) = self.error.clone() {
            egui::Modal::new(egui::Id::new("error")).show(ctx, |ui| {
                ui.set_width(480.0);
                ui.heading("Could not complete that action");
                ui.label(message);
                if ui.button("OK").clicked() {
                    self.error = None;
                }
            });
        }
        if self.show_export {
            egui::Modal::new(egui::Id::new("export")).show(ctx,|ui| {
                ui.set_width(460.0);
                ui.heading("Export PNG + CSV");
                ui.label("Creates a new folder containing tile artwork, individual slices, and origin-relative layout data.");
                ui.horizontal(|ui| {
                    ui.label("Hardware object size");
                    if ui.selectable_value(&mut self.object_height,8,"8 × 8").changed() { self.warnings_dirty = true; }
                    if ui.selectable_value(&mut self.object_height,16,"8 × 16").changed() { self.warnings_dirty = true; }
                });
                ui.add(egui::DragValue::new(&mut self.tiles_per_row).range(2..=64).speed(2).prefix("8×8 tiles per row: "));
                if self.object_height==16 { ui.label("8×16 objects use consecutive top/bottom tiles with an even starting index."); }
                ui.label("Original RGB colors are preserved. Color filtering and the transparent background are applied.");
                if !self.warnings.is_empty() { ui.colored_label(Color32::from_rgb(239,191,112),format!("{} hardware warning(s). Review Hardware check before conversion.",self.warnings.len())); }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Choose parent folder and export…").clicked() && let Some(parent)=rfd::FileDialog::new().pick_folder() {
                        let destination = unique_export_path(&parent);
                        if let Some(image) = &self.source {
                            match export::export_project(&self.project,image,&destination,export::ExportOptions {object_height:self.object_height,tiles_per_row:self.tiles_per_row}) {
                                Ok(summary) => { self.status = format!("Exported {} slices / {} objects / {} tiles → {}",summary.slices,summary.objects,summary.tiles,destination.display()); self.show_export = false; }
                                Err(error) => { self.error = Some(error); self.show_export = false; }
                            }
                        }
                    }
                    if ui.button("Cancel").clicked() { self.show_export = false; }
                });
            });
        }
        if self.show_help {
            egui::Window::new("Quick guide").open(&mut self.show_help).resizable(false).default_width(500.0).show(ctx,|ui| {
                ui.label("1. Open or drop a PNG. Enable hot reload to keep painting elsewhere.");
                ui.label("2. Pick a slice size, then click anywhere to place it. Overlap is allowed.");
                ui.label("3. Select a slice to drag it, edit exact coordinates, or filter its colors.");
                ui.label("4. Add a cel for each animation frame. Set its origin with the crosshair tool.");
                ui.label("5. Save the CSV project, then export PNG tiles and layout CSVs.");
                ui.separator();
                for text in ["V: select/move · B: place slice · O: place cel origin", "Arrow keys: nudge 1 pixel · Shift + arrows: nudge 8 pixels", "Ctrl/Cmd + D: duplicate in place · Delete: remove selected slice", "Ctrl/Cmd + O: open · S: save · Shift + S: save as", "Ctrl/Cmd + Z: undo · Shift + Z or Y: redo", "Scroll / right or middle drag: pan · Ctrl/Cmd + scroll: zoom", "Use the slice list to select a slice hidden under an overlapping slice."] { ui.label(text); }
                ui.separator();
                ui.label("Hardware notes: one global OBJ size per export; 3 opaque colors + transparency per object; GB palettes 0–1, GBC 0–7. Exported coordinates are logical positions, without OAM +8/+16 offsets. Custom sizes are padded to hardware objects. PNG conversion, tile banking and palette assignment remain with your game tools.");
            });
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let before = self.project.clone();
        let epoch = self.epoch;
        #[cfg(feature = "screenshot")]
        if let Ok(path) = std::env::var("GBC_SCREENSHOT_TO") {
            if let Some(image) = ctx.input(|input| {
                input.events.iter().find_map(|event| {
                    if let egui::Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            }) {
                let rgba: Vec<u8> = image
                    .pixels
                    .iter()
                    .flat_map(|color| color.to_array())
                    .collect();
                if let Err(error) = image::save_buffer(
                    &path,
                    &rgba,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image::ColorType::Rgba8,
                ) {
                    eprintln!("Screenshot: {error}");
                }
                self.close_allowed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if ctx.cumulative_frame_nr() >= 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            ctx.request_repaint();
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.close_allowed && self.dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        if (self.hot_reload || self.source.is_none())
            && self.last_poll.elapsed() >= Duration::from_millis(500)
        {
            self.last_poll = Instant::now();
            let stamp = file_stamp(Path::new(&self.project.image_path));
            if !self.project.image_path.is_empty()
                && (stamp != self.source_stamp || self.source.is_none())
            {
                self.reload(&ctx, false);
            }
        }
        if self.hot_reload || self.source.is_none() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        let drops: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(path) = drops.first() {
            self.request(&ctx, Pending::Open(path.clone()));
        }
        self.shortcuts(&ctx);
        egui::Panel::top("toolbar").show_inside(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show_inside(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&self.status).small().color(MUTED));
                if let Some((x, y)) = self.hover_pixel {
                    ui.label(
                        RichText::new(format!("   X {x}  Y {y}"))
                            .small()
                            .color(ACCENT),
                    );
                }
            });
            ui.label(
                RichText::new(match self.tool {
                    Tool::Slice => "Click to place · V to move · Right drag / scroll to pan",
                    Tool::Select => "Drag to move · Arrow keys to nudge · Ctrl/Cmd+D to duplicate",
                    Tool::Origin => "Click to set this cel’s origin",
                    Tool::Background => {
                        "Click a source pixel to make that RGB color transparent on export"
                    }
                })
                .small(),
            );
        });
        egui::Panel::left("project")
            .default_size(236.0)
            .min_size(200.0)
            .max_size(360.0)
            .show_inside(ui, |ui| self.project_panel(ui));
        egui::Panel::right("inspector")
            .default_size(266.0)
            .min_size(235.0)
            .max_size(400.0)
            .show_inside(ui, |ui| self.inspector(ui));
        egui::CentralPanel::default().show_inside(ui, |ui| self.canvas(ui));
        self.dialogs(&ctx);
        if self.epoch == epoch && before != self.project {
            self.warnings_dirty = true;
            if ctx.input(|i| i.pointer.primary_down()) {
                if self.gesture_base.is_none() {
                    self.gesture_base = Some(before);
                }
            } else if self.gesture_base.is_none() {
                self.push_undo(before);
            }
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.finish_gesture();
        }
        if self.warnings_dirty {
            self.warnings = self
                .source
                .as_ref()
                .map(|image| export::warnings(&self.project, image, self.object_height))
                .unwrap_or_default();
            self.warnings_dirty = false;
        }
        let name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{}{name} — GBC Sprite Editor",
            if self.dirty() { "* " } else { "" }
        )));
    }
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().strong().color(MUTED));
}
fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| Some((m.modified().ok()?, m.len())))
}
fn pixel_at(point: Pos2, origin: Pos2, zoom: f32) -> (i32, i32) {
    (
        ((point.x - origin.x) / zoom).floor() as i32,
        ((point.y - origin.y) / zoom).floor() as i32,
    )
}
fn snapped(value: i32, step: i32) -> i32 {
    (value as f32 / step as f32).round() as i32 * step
}
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => std::path::absolute(a).ok() == std::path::absolute(b).ok(),
    }
}
fn unique_export_path(parent: &Path) -> PathBuf {
    let base = parent.join("sprites-export");
    if !base.exists() {
        return base;
    }
    for n in 2.. {
        let path = parent.join(format!("sprites-export-{n}"));
        if !path.exists() {
            return path;
        }
    }
    unreachable!()
}
fn swatch(ui: &mut egui::Ui, rgb: [u8; 3], active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(25.0), Sense::click());
    ui.painter().rect_filled(
        rect.shrink(3.0),
        3.0,
        Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
    );
    ui.painter().rect_stroke(
        rect,
        3.0,
        Stroke::new(
            if active { 2.0_f32 } else { 1.0_f32 },
            if active {
                ACCENT
            } else {
                Color32::from_gray(70)
            },
        ),
        egui::StrokeKind::Inside,
    );
    if !active {
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_top()],
            Stroke::new(1.0_f32, Color32::WHITE),
        );
    }
    response.on_hover_text(format!(
        "#{:02X}{:02X}{:02X} · {}",
        rgb[0],
        rgb[1],
        rgb[2],
        if active { "kept" } else { "removed" }
    ))
}
fn checker(painter: &egui::Painter, rect: Rect, cell: f32) {
    if !rect.is_positive() {
        return;
    }
    painter.rect_filled(rect, 0.0, Color32::from_rgb(38, 44, 54));
    for row in 0..(rect.height() / cell).ceil() as i32 {
        for col in 0..(rect.width() / cell).ceil() as i32 {
            if (row + col) % 2 == 0 {
                let square = Rect::from_min_size(
                    rect.min + Vec2::new(col as f32, row as f32) * cell,
                    Vec2::splat(cell),
                )
                .intersect(rect);
                painter.rect_filled(square, 0.0, Color32::from_rgb(47, 54, 65));
            }
        }
    }
}
fn source_colors(image: &image::RgbaImage, slice: &Slice, key: Option<[u8; 3]>) -> Vec<[u8; 3]> {
    let mut colors = BTreeSet::new();
    for y in slice.y.max(0)..(slice.y + slice.height as i32).min(image.height() as i32) {
        for x in slice.x.max(0)..(slice.x + slice.width as i32).min(image.width() as i32) {
            let pixel = image.get_pixel(x as u32, y as u32).0;
            let rgb = [pixel[0], pixel[1], pixel[2]];
            if pixel[3] > 0 && Some(rgb) != key {
                colors.insert(rgb);
            }
        }
    }
    colors.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canvas_coordinates_keep_non_grid_offsets_and_negative_positions() {
        assert_eq!(
            pixel_at(Pos2::new(113.0, 137.0), Pos2::new(100.0, 100.0), 4.0),
            (3, 9)
        );
        assert_eq!(
            pixel_at(Pos2::new(99.0, 99.0), Pos2::new(100.0, 100.0), 4.0),
            (-1, -1)
        );
        assert_eq!(snapped(-9, 8), -8);
    }
    #[test]
    fn duplicate_preserves_layer_geometry_and_undo_restores() {
        let mut editor = Editor::empty();
        editor.cel_mut().slices.push(Slice {
            x: 3,
            y: 7,
            colors: Some(vec![[12, 34, 56]]),
            ..Slice::default()
        });
        editor.selected_slice = Some(0);
        let before = editor.project.clone();
        editor.duplicate();
        editor.push_undo(before.clone());
        assert_eq!(editor.cel().slices[1].x, 3);
        assert_eq!(editor.cel().slices[1].colors, Some(vec![[12, 34, 56]]));
        editor.undo(false);
        assert_eq!(editor.project, before);
        editor.undo(true);
        assert_eq!(editor.cel().slices.len(), 2);
    }
    #[test]
    fn headless_editor_draws_empty_and_loaded_documents() {
        let ctx = egui::Context::default();
        Editor::configure(&ctx);
        let mut editor = Editor::empty();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 840.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input.clone(), |ui| editor.draw(ui));
        editor.install_source(
            &ctx,
            image::RgbaImage::from_pixel(32, 32, image::Rgba([80, 100, 120, 255])),
        );
        editor.cel_mut().slices.push(Slice::default());
        editor.selected_slice = Some(0);
        let output = ctx.run_ui(input, |ui| editor.draw(ui));
        assert!(!output.shapes.is_empty());
    }
    #[test]
    fn pointer_can_place_then_drag_a_slice_beyond_its_original_bounds() {
        let ctx = egui::Context::default();
        let mut editor = Editor::empty();
        editor.install_source(&ctx, image::RgbaImage::new(64, 64));
        editor.fit = false;
        editor.zoom = 4.0;
        let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| editor.canvas(ui));
        };
        let pointer = |pos: Pos2, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut editor, vec![]);
        let start = Pos2::new(32.0, 100.0);
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(start), pointer(start, true)],
        );
        frame(&mut editor, vec![pointer(start, false)]);
        assert_eq!(editor.cel().slices.len(), 1);
        let original = editor.cel().slices[0].clone();
        editor.tool = Tool::Select;
        frame(&mut editor, vec![pointer(start, true)]);
        let moved = start + Vec2::new(80.0, 0.0);
        frame(&mut editor, vec![egui::Event::PointerMoved(moved)]);
        frame(&mut editor, vec![pointer(moved, false)]);
        assert_eq!(editor.cel().slices[0].x, original.x + 20);
        assert_eq!(editor.cel().slices[0].y, original.y);
    }
    #[test]
    fn bad_png_keeps_current_project_and_empty_project_reopens_without_error() {
        let ctx = egui::Context::default();
        let mut editor = Editor::empty();
        editor.cel_mut().slices.push(Slice::default());
        let before = editor.project.clone();
        let folder = tempfile::tempdir().unwrap();
        let bad = folder.path().join("broken.png");
        std::fs::write(&bad, b"invalid png").unwrap();
        editor.open(&ctx, &bad);
        assert_eq!(editor.project, before);
        assert!(editor.error.take().is_some());
        let empty = folder.path().join("empty.gbcs");
        project::save(&Project::default(), &empty).unwrap();
        editor.open(&ctx, &empty);
        assert!(editor.error.is_none());
        assert!(editor.source.is_none());
        assert!(!editor.dirty());
    }
}
