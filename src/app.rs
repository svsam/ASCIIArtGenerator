use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use ascii_art_generator::{
    AsciiDocument, BatchOutputPaths, CharacterRamp, ConversionSettings, CropRect, DitherMode,
    ExportFormats, RowSizing, atomic_write, batch_output_paths, convert, decode_image, render_ansi,
    render_plain,
};
use eframe::egui::{self, Color32, FontId, RichText, Sense, TextFormat, TextureHandle, Vec2};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

const STORAGE_KEY: &str = "ascii-art-generator-state-v1";
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedPreset {
    name: String,
    settings: ConversionSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistentState {
    shared_settings: ConversionSettings,
    saved_presets: Vec<SavedPreset>,
    dark_mode: bool,
    last_export_directory: Option<PathBuf>,
    export_formats: PersistedFormats,
    color_preview: bool,
    ansi_dark_background: bool,
    preview_font_size: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PersistedFormats {
    plain: bool,
    ansi: bool,
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            shared_settings: ConversionSettings::default(),
            saved_presets: Vec::new(),
            dark_mode: true,
            last_export_directory: None,
            export_formats: PersistedFormats {
                plain: true,
                ansi: false,
            },
            color_preview: false,
            ansi_dark_background: true,
            preview_font_size: 13.0,
        }
    }
}

struct ImageItem {
    id: u64,
    path: PathBuf,
    image: Option<Arc<RgbaImage>>,
    texture: Option<TextureHandle>,
    dimensions: Option<(u32, u32)>,
    crop: CropRect,
    settings_override: Option<ConversionSettings>,
    document: Option<Arc<AsciiDocument>>,
    error: Option<String>,
    loading: bool,
    converting: bool,
    generation: u64,
    dirty_since: Option<Instant>,
}

impl ImageItem {
    fn label(&self) -> String {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Unnamed image")
            .to_owned()
    }
}

enum Job {
    Load {
        id: u64,
        path: PathBuf,
    },
    Convert {
        id: u64,
        generation: u64,
        image: Arc<RgbaImage>,
        crop: CropRect,
        settings: ConversionSettings,
    },
    Export(ExportJob),
}

struct ExportJob {
    id: u64,
    image: Arc<RgbaImage>,
    crop: CropRect,
    settings: ConversionSettings,
    paths: BatchOutputPaths,
    cancel: Arc<AtomicBool>,
}

enum JobResult {
    Loaded {
        id: u64,
        result: Result<LoadedImage, String>,
    },
    Converted {
        id: u64,
        generation: u64,
        result: Result<AsciiDocument, String>,
    },
    Exported {
        id: u64,
        result: Result<(), String>,
    },
}

struct LoadedImage {
    image: Arc<RgbaImage>,
    preview: RgbaImage,
}

struct PendingBatch {
    paths: Vec<BatchOutputPaths>,
    conflict_count: usize,
}

struct ExportProgress {
    total: usize,
    finished: usize,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy)]
enum CropDragKind {
    Move,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

struct CropDrag {
    item_id: u64,
    kind: CropDragKind,
    start_crop: CropRect,
    start_pointer: egui::Pos2,
}

pub struct AsciiArtApp {
    persistent: PersistentState,
    items: Vec<ImageItem>,
    selected_id: Option<u64>,
    next_id: u64,
    job_sender: Sender<Job>,
    result_receiver: Receiver<JobResult>,
    status: String,
    new_preset_name: String,
    selected_saved_preset: Option<usize>,
    pending_batch: Option<PendingBatch>,
    export_progress: Option<ExportProgress>,
    crop_drag: Option<CropDrag>,
}

impl AsciiArtApp {
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        let persistent: PersistentState = creation_context
            .storage
            .and_then(|storage| eframe::get_value(storage, STORAGE_KEY))
            .unwrap_or_default();
        if persistent.dark_mode {
            creation_context.egui_ctx.set_visuals(egui::Visuals::dark());
        } else {
            creation_context
                .egui_ctx
                .set_visuals(egui::Visuals::light());
        }
        let (job_sender, result_receiver) = spawn_workers(creation_context.egui_ctx.clone());
        Self {
            persistent,
            items: Vec::new(),
            selected_id: None,
            next_id: 1,
            job_sender,
            result_receiver,
            status: "Drop images here or choose Open images".to_owned(),
            new_preset_name: String::new(),
            selected_saved_preset: None,
            pending_batch: None,
            export_progress: None,
            crop_drag: None,
        }
    }

    fn export_formats(&self) -> ExportFormats {
        ExportFormats {
            plain: self.persistent.export_formats.plain,
            ansi: self.persistent.export_formats.ansi,
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected_id?;
        self.items.iter().position(|item| item.id == selected)
    }

    fn effective_settings(&self, index: usize) -> ConversionSettings {
        self.items[index]
            .settings_override
            .clone()
            .unwrap_or_else(|| self.persistent.shared_settings.clone())
    }

    fn mark_item_dirty(&mut self, index: usize, context: &egui::Context) {
        let item = &mut self.items[index];
        item.generation = item.generation.wrapping_add(1);
        item.dirty_since = Some(Instant::now());
        context.request_repaint_after(PREVIEW_DEBOUNCE + Duration::from_millis(10));
    }

    fn mark_shared_items_dirty(&mut self, context: &egui::Context) {
        let now = Instant::now();
        for item in &mut self.items {
            if item.settings_override.is_none() {
                item.generation = item.generation.wrapping_add(1);
                item.dirty_since = Some(now);
            }
        }
        context.request_repaint_after(PREVIEW_DEBOUNCE + Duration::from_millis(10));
    }

    fn queue_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !is_supported_image(&path) {
                self.status = format!("Unsupported image type: {}", path.display());
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            self.items.push(ImageItem {
                id,
                path: path.clone(),
                image: None,
                texture: None,
                dimensions: None,
                crop: CropRect::FULL,
                settings_override: None,
                document: None,
                error: None,
                loading: true,
                converting: false,
                generation: 0,
                dirty_since: None,
            });
            self.selected_id.get_or_insert(id);
            if self.job_sender.send(Job::Load { id, path }).is_err() {
                self.status = "The image worker stopped unexpectedly".to_owned();
            }
        }
    }

    fn open_images(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter(
            "Images",
            &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"],
        );
        if let Some(directory) = self.persistent.last_export_directory.as_ref() {
            dialog = dialog.set_directory(directory);
        }
        if let Some(paths) = dialog.pick_files() {
            self.queue_paths(paths);
        }
    }

    fn remove_selected(&mut self) {
        if let Some(index) = self.selected_index() {
            self.items.remove(index);
            self.selected_id = self
                .items
                .get(index.min(self.items.len().saturating_sub(1)))
                .map(|item| item.id);
        }
    }

    fn move_selected(&mut self, offset: isize) {
        let Some(index) = self.selected_index() else {
            return;
        };
        let destination =
            (index as isize + offset).clamp(0, self.items.len() as isize - 1) as usize;
        if destination != index {
            self.items.swap(index, destination);
        }
    }

    fn process_results(&mut self, context: &egui::Context) {
        while let Ok(result) = self.result_receiver.try_recv() {
            match result {
                JobResult::Loaded { id, result } => {
                    let Some(index) = self.items.iter().position(|item| item.id == id) else {
                        continue;
                    };
                    let item = &mut self.items[index];
                    item.loading = false;
                    match result {
                        Ok(loaded) => {
                            let size = [
                                loaded.preview.width() as usize,
                                loaded.preview.height() as usize,
                            ];
                            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                                size,
                                loaded.preview.as_raw(),
                            );
                            item.texture = Some(context.load_texture(
                                format!("source-image-{id}"),
                                color_image,
                                egui::TextureOptions::LINEAR,
                            ));
                            item.dimensions = Some((loaded.image.width(), loaded.image.height()));
                            item.image = Some(loaded.image);
                            item.error = None;
                            self.status = format!("Loaded {}", item.label());
                            self.mark_item_dirty(index, context);
                        }
                        Err(error) => {
                            item.error = Some(error.clone());
                            self.status = error;
                        }
                    }
                }
                JobResult::Converted {
                    id,
                    generation,
                    result,
                } => {
                    let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
                        continue;
                    };
                    if generation != item.generation {
                        continue;
                    }
                    item.converting = false;
                    match result {
                        Ok(document) => {
                            item.document = Some(Arc::new(document));
                            item.error = None;
                        }
                        Err(error) => {
                            item.error = Some(error.clone());
                            self.status = error;
                        }
                    }
                }
                JobResult::Exported { id, result } => {
                    if let Some(progress) = self.export_progress.as_mut() {
                        progress.finished += 1;
                    }
                    match result {
                        Ok(()) => self.status = "Export completed".to_owned(),
                        Err(error) if error == "Export cancelled" => {
                            self.status = "Export cancelled".to_owned();
                        }
                        Err(error) => {
                            if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                                item.error = Some(error.clone());
                            }
                            self.status = error;
                        }
                    }
                    if self
                        .export_progress
                        .as_ref()
                        .is_some_and(|progress| progress.finished >= progress.total)
                    {
                        self.export_progress = None;
                    }
                }
            }
        }
    }

    fn schedule_previews(&mut self) {
        let now = Instant::now();
        for index in 0..self.items.len() {
            let should_schedule = self.items[index]
                .dirty_since
                .is_some_and(|dirty| now.duration_since(dirty) >= PREVIEW_DEBOUNCE)
                && self.items[index].image.is_some();
            if !should_schedule {
                continue;
            }
            let settings = self.effective_settings(index);
            let item = &mut self.items[index];
            let image = item.image.as_ref().expect("checked above").clone();
            item.dirty_since = None;
            item.converting = true;
            let job = Job::Convert {
                id: item.id,
                generation: item.generation,
                image,
                crop: item.crop,
                settings,
            };
            if self.job_sender.send(job).is_err() {
                item.converting = false;
                item.error = Some("The conversion worker stopped unexpectedly".to_owned());
            }
        }
    }

    fn export_selected(&mut self) {
        let Some(index) = self.selected_index() else {
            self.status = "Select an image before exporting".to_owned();
            return;
        };
        let formats = self.export_formats();
        if !formats.plain && !formats.ansi {
            self.status = "Select at least one export format".to_owned();
            return;
        }
        let item = &self.items[index];
        let Some(image) = item.image.clone() else {
            self.status = "The selected image is still loading".to_owned();
            return;
        };
        let stem = item
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("image");
        let suggested = if formats.plain {
            format!("{stem}_ascii.txt")
        } else {
            format!("{stem}_ascii.ansi.txt")
        };
        let mut dialog = rfd::FileDialog::new().set_file_name(&suggested);
        if let Some(directory) = self.persistent.last_export_directory.as_ref() {
            dialog = dialog.set_directory(directory);
        }
        let Some(chosen) = dialog.add_filter("Text", &["txt"]).save_file() else {
            return;
        };
        self.persistent.last_export_directory = chosen.parent().map(Path::to_path_buf);
        let paths = selected_output_paths(chosen, formats);
        self.start_exports(vec![(index, image, paths)]);
    }

    fn prepare_batch_export(&mut self) {
        let formats = self.export_formats();
        if !formats.plain && !formats.ansi {
            self.status = "Select at least one export format".to_owned();
            return;
        }
        let ready: Vec<_> = self
            .items
            .iter()
            .filter(|item| item.image.is_some())
            .map(|item| item.path.clone())
            .collect();
        if ready.is_empty() {
            self.status = "Load at least one image before exporting".to_owned();
            return;
        }
        let mut dialog = rfd::FileDialog::new();
        if let Some(directory) = self.persistent.last_export_directory.as_ref() {
            dialog = dialog.set_directory(directory);
        }
        let Some(directory) = dialog.pick_folder() else {
            return;
        };
        self.persistent.last_export_directory = Some(directory.clone());
        let paths = batch_output_paths(&ready, &directory, formats);
        let conflict_count = paths
            .iter()
            .flat_map(|paths| [paths.plain.as_ref(), paths.ansi.as_ref()])
            .flatten()
            .filter(|path| path.exists())
            .count();
        if conflict_count > 0 {
            self.pending_batch = Some(PendingBatch {
                paths,
                conflict_count,
            });
        } else {
            self.queue_prepared_batch(paths, false);
        }
    }

    fn queue_prepared_batch(&mut self, paths: Vec<BatchOutputPaths>, skip_existing: bool) {
        let ready_indices: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| item.image.as_ref().map(|_| index))
            .collect();
        let jobs = ready_indices
            .into_iter()
            .zip(paths)
            .filter_map(|(index, mut paths)| {
                if skip_existing {
                    if paths.plain.as_ref().is_some_and(|path| path.exists()) {
                        paths.plain = None;
                    }
                    if paths.ansi.as_ref().is_some_and(|path| path.exists()) {
                        paths.ansi = None;
                    }
                }
                if paths.plain.is_none() && paths.ansi.is_none() {
                    return None;
                }
                Some((index, self.items[index].image.as_ref()?.clone(), paths))
            })
            .collect();
        self.start_exports(jobs);
    }

    fn start_exports(&mut self, jobs: Vec<(usize, Arc<RgbaImage>, BatchOutputPaths)>) {
        if jobs.is_empty() {
            self.status = "No files needed exporting".to_owned();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let total = jobs.len();
        for (index, image, paths) in jobs {
            let job = ExportJob {
                id: self.items[index].id,
                image,
                crop: self.items[index].crop,
                settings: self.effective_settings(index),
                paths,
                cancel: cancel.clone(),
            };
            if self.job_sender.send(Job::Export(job)).is_err() {
                self.status = "The export worker stopped unexpectedly".to_owned();
                return;
            }
        }
        self.export_progress = Some(ExportProgress {
            total,
            finished: 0,
            cancel,
        });
        self.status = format!("Exporting {total} image(s)…");
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut open = false;
        let mut export_selected = false;
        let mut export_all = false;
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("ASCII Art Generator");
                ui.separator();
                open = ui.button("Open images…  Ctrl+O").clicked();
                export_selected = ui
                    .add_enabled(
                        self.selected_id.is_some(),
                        egui::Button::new("Export selected…  Ctrl+S"),
                    )
                    .clicked();
                export_all = ui
                    .add_enabled(!self.items.is_empty(), egui::Button::new("Export all…"))
                    .clicked();
                ui.separator();
                ui.checkbox(&mut self.persistent.export_formats.plain, "Plain .txt");
                ui.checkbox(&mut self.persistent.export_formats.ansi, "ANSI colour");
                ui.separator();
                if ui
                    .button(if self.persistent.dark_mode {
                        "Light theme"
                    } else {
                        "Dark theme"
                    })
                    .clicked()
                {
                    self.persistent.dark_mode = !self.persistent.dark_mode;
                    ui.ctx().set_visuals(if self.persistent.dark_mode {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                }
            });
            if let Some(progress) = self.export_progress.as_ref() {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::ProgressBar::new(progress.finished as f32 / progress.total as f32)
                            .text(format!(
                                "Exported {} of {}",
                                progress.finished, progress.total
                            )),
                    );
                    if ui.button("Cancel").clicked() {
                        progress.cancel.store(true, Ordering::Relaxed);
                    }
                });
            }
            let mut conflict_action = None;
            let mut cancel_conflict = false;
            if let Some(conflict_count) = self
                .pending_batch
                .as_ref()
                .map(|pending| pending.conflict_count)
            {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        Color32::YELLOW,
                        format!("{conflict_count} output file(s) already exist."),
                    );
                    if ui.button("Overwrite all").clicked() {
                        conflict_action = Some(false);
                    }
                    if ui.button("Skip existing").clicked() {
                        conflict_action = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_conflict = true;
                    }
                });
            }
            if cancel_conflict {
                self.pending_batch = None;
            }
            if let Some(skip_existing) = conflict_action
                && let Some(pending) = self.pending_batch.take()
            {
                self.queue_prepared_batch(pending.paths, skip_existing);
            }
            ui.small(&self.status);
        });
        if open {
            self.open_images();
        }
        if export_selected {
            self.export_selected();
        }
        if export_all {
            self.prepare_batch_export();
        }
    }

    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        let mut select = None;
        let mut remove = false;
        let mut move_by = 0;
        egui::Panel::left("image-queue")
            .default_size(230.0)
            .min_size(170.0)
            .max_size(360.0)
            .resizable(true)
            .show(ui, |ui| {
                ui.heading("Image queue");
                ui.horizontal(|ui| {
                    if ui.button("↑").on_hover_text("Move up").clicked() {
                        move_by = -1;
                    }
                    if ui.button("↓").on_hover_text("Move down").clicked() {
                        move_by = 1;
                    }
                    remove = ui
                        .add_enabled(self.selected_id.is_some(), egui::Button::new("Remove"))
                        .clicked();
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for item in &self.items {
                        let state = if item.loading {
                            "loading"
                        } else if item.converting {
                            "updating"
                        } else if item.error.is_some() {
                            "error"
                        } else {
                            "ready"
                        };
                        let label = format!("{}\n  {state}", item.label());
                        if ui
                            .selectable_label(self.selected_id == Some(item.id), label)
                            .clicked()
                        {
                            select = Some(item.id);
                        }
                    }
                });
                if self.items.is_empty() {
                    ui.add_space(20.0);
                    ui.label("Drop PNG, JPEG, GIF, WebP, BMP, or TIFF images into the window.");
                }
            });
        if let Some(id) = select {
            self.selected_id = Some(id);
        }
        if remove {
            self.remove_selected();
        }
        if move_by != 0 {
            self.move_selected(move_by);
        }
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        egui::Panel::right("settings")
            .default_size(310.0)
            .min_size(260.0)
            .max_size(420.0)
            .resizable(true)
            .show(ui, |ui| {
                ui.heading("Conversion settings");
                let selected_index = self.selected_index();
                let mut apply_to_all = false;
                if let Some(index) = selected_index {
                    let uses_override = self.items[index].settings_override.is_some();
                    ui.horizontal(|ui| {
                        ui.label(if uses_override {
                            "Per-image override"
                        } else {
                            "Shared settings"
                        });
                        if uses_override {
                            if ui.button("Reset to shared").clicked() {
                                self.items[index].settings_override = None;
                                self.mark_item_dirty(index, &context);
                            }
                        } else if ui.button("Create override").clicked() {
                            self.items[index].settings_override =
                                Some(self.persistent.shared_settings.clone());
                        }
                        apply_to_all = ui
                            .button("Apply to all")
                            .on_hover_text(
                                "Make these settings shared and clear every per-image override",
                            )
                            .clicked();
                    });
                } else {
                    ui.label("Shared settings");
                }
                if apply_to_all {
                    let settings = self.effective_settings(selected_index.expect("selected above"));
                    self.persistent.shared_settings = settings;
                    for item in &mut self.items {
                        item.settings_override = None;
                    }
                    self.mark_shared_items_dirty(&context);
                }
                ui.separator();

                let editing_override = selected_index
                    .is_some_and(|index| self.items[index].settings_override.is_some());
                let changed = if editing_override {
                    let index = selected_index.expect("checked above");
                    settings_editor(
                        ui,
                        self.items[index]
                            .settings_override
                            .as_mut()
                            .expect("checked above"),
                    )
                } else {
                    settings_editor(ui, &mut self.persistent.shared_settings)
                };
                if changed {
                    if editing_override {
                        self.mark_item_dirty(selected_index.expect("checked above"), &context);
                    } else {
                        self.mark_shared_items_dirty(&context);
                    }
                }

                ui.separator();
                ui.heading("Presets");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_preset_name)
                            .hint_text("Preset name"),
                    );
                    if ui.button("Save").clicked() {
                        let name = self.new_preset_name.trim();
                        if !name.is_empty() {
                            let settings = selected_index
                                .map(|index| self.effective_settings(index))
                                .unwrap_or_else(|| self.persistent.shared_settings.clone());
                            if let Some(existing) = self
                                .persistent
                                .saved_presets
                                .iter_mut()
                                .find(|preset| preset.name == name)
                            {
                                existing.settings = settings;
                            } else {
                                self.persistent.saved_presets.push(SavedPreset {
                                    name: name.to_owned(),
                                    settings,
                                });
                            }
                            self.new_preset_name.clear();
                        }
                    }
                });
                if !self.persistent.saved_presets.is_empty() {
                    let selected_text = self
                        .selected_saved_preset
                        .and_then(|index| self.persistent.saved_presets.get(index))
                        .map(|preset| preset.name.as_str())
                        .unwrap_or("Choose saved preset");
                    egui::ComboBox::from_id_salt("saved-presets")
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            for (index, preset) in self.persistent.saved_presets.iter().enumerate()
                            {
                                ui.selectable_value(
                                    &mut self.selected_saved_preset,
                                    Some(index),
                                    &preset.name,
                                );
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked()
                            && let Some(settings) = self
                                .selected_saved_preset
                                .and_then(|index| self.persistent.saved_presets.get(index))
                                .map(|preset| preset.settings.clone())
                        {
                            if let Some(index) = selected_index
                                && self.items[index].settings_override.is_some()
                            {
                                self.items[index].settings_override = Some(settings);
                                self.mark_item_dirty(index, &context);
                            } else {
                                self.persistent.shared_settings = settings;
                                self.mark_shared_items_dirty(&context);
                            }
                        }
                        if ui.button("Delete").clicked()
                            && let Some(index) = self.selected_saved_preset.take()
                            && index < self.persistent.saved_presets.len()
                        {
                            self.persistent.saved_presets.remove(index);
                        }
                    });
                }
            });
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(index) = self.selected_index() else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Open or drop an image to begin").size(24.0));
                });
                return;
            };
            let item_id = self.items[index].id;
            let texture = self.items[index].texture.clone();
            let crop = self.items[index].crop;
            let document = self.items[index].document.clone();
            let error = self.items[index].error.clone();
            let dimensions = self.items[index].dimensions;

            ui.horizontal(|ui| {
                ui.heading(self.items[index].label());
                if let Some((width, height)) = dimensions {
                    ui.label(format!("{width} × {height}px"));
                }
                if ui.button("Reset crop").clicked() {
                    self.items[index].crop = CropRect::FULL;
                    self.mark_item_dirty(index, ui.ctx());
                }
            });
            if let Some(error) = error {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
            ui.separator();

            let available_height = ui.available_height();
            let source_height = (available_height * 0.42).max(180.0);
            ui.allocate_ui(Vec2::new(ui.available_width(), source_height), |ui| {
                ui.heading("Source and crop");
                if let Some(texture) = texture.as_ref() {
                    self.crop_editor(ui, item_id, texture, crop);
                } else {
                    ui.spinner();
                    ui.label("Decoding image…");
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.heading("ASCII preview");
                ui.selectable_value(&mut self.persistent.color_preview, false, "Monochrome");
                ui.selectable_value(&mut self.persistent.color_preview, true, "ANSI colour");
                if self.persistent.color_preview {
                    ui.checkbox(&mut self.persistent.ansi_dark_background, "Dark background");
                }
                ui.add(
                    egui::Slider::new(&mut self.persistent.preview_font_size, 8.0..=28.0)
                        .text("Zoom"),
                );
                if let Some(document) = document.as_ref() {
                    if ui.button("Copy").clicked() {
                        let text = if self.persistent.color_preview {
                            render_ansi(document)
                        } else {
                            render_plain(document)
                        };
                        ui.ctx().copy_text(text);
                        self.status = "Copied preview to the clipboard".to_owned();
                    }
                    ui.label(format!("{} × {}", document.width, document.height));
                }
            });
            if let Some(document) = document.as_ref() {
                ascii_preview(
                    ui,
                    document,
                    self.persistent.color_preview,
                    self.persistent.ansi_dark_background,
                    self.persistent.preview_font_size,
                );
            } else {
                ui.spinner();
                ui.label("Generating preview…");
            }
        });
    }

    fn crop_editor(
        &mut self,
        ui: &mut egui::Ui,
        item_id: u64,
        texture: &TextureHandle,
        crop: CropRect,
    ) {
        let available = ui.available_size();
        let aspect = texture.aspect_ratio();
        let mut size = Vec2::new(available.x.max(1.0), available.x.max(1.0) / aspect);
        if size.y > available.y {
            size.y = available.y.max(1.0);
            size.x = size.y * aspect;
        }
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        egui::Image::from_texture(texture)
            .fit_to_exact_size(size)
            .paint_at(ui, rect);

        let crop_rect = normalized_to_screen(rect, crop);
        let painter = ui.painter();
        let stroke = egui::Stroke::new(2.0, Color32::from_rgb(77, 208, 225));
        painter.line_segment([crop_rect.left_top(), crop_rect.right_top()], stroke);
        painter.line_segment([crop_rect.right_top(), crop_rect.right_bottom()], stroke);
        painter.line_segment([crop_rect.right_bottom(), crop_rect.left_bottom()], stroke);
        painter.line_segment([crop_rect.left_bottom(), crop_rect.left_top()], stroke);
        for point in [
            crop_rect.left_top(),
            crop_rect.right_top(),
            crop_rect.left_bottom(),
            crop_rect.right_bottom(),
        ] {
            painter.circle_filled(point, 5.0, Color32::WHITE);
            painter.circle_stroke(point, 5.0, stroke);
        }

        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let kind = crop_drag_kind(crop_rect, pointer);
            self.crop_drag = Some(CropDrag {
                item_id,
                kind,
                start_crop: crop,
                start_pointer: pointer,
            });
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(drag) = self.crop_drag.as_ref()
            && drag.item_id == item_id
        {
            let delta = Vec2::new(
                (pointer.x - drag.start_pointer.x) / rect.width(),
                (pointer.y - drag.start_pointer.y) / rect.height(),
            );
            let updated = apply_crop_drag(drag.start_crop, drag.kind, delta);
            if let Some(index) = self.items.iter().position(|item| item.id == item_id)
                && updated != self.items[index].crop
            {
                self.items[index].crop = updated;
                self.mark_item_dirty(index, ui.ctx());
            }
        }
        if response.drag_stopped() {
            self.crop_drag = None;
        }
    }

    fn handle_input(&mut self, context: &egui::Context) {
        let dropped = context.input(|input| input.raw.dropped_files.clone());
        let paths = dropped.into_iter().filter_map(|file| file.path);
        self.queue_paths(paths);

        let (open, save, delete) = context.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::O),
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.key_pressed(egui::Key::Delete),
            )
        });
        if open {
            self.open_images();
        }
        if save {
            self.export_selected();
        }
        if delete && !context.text_edit_focused() {
            self.remove_selected();
        }
    }
}

impl eframe::App for AsciiArtApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_results(context);
        self.schedule_previews();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_input(ui.ctx());
        self.toolbar(ui);
        self.queue_panel(ui);
        self.settings_panel(ui);
        self.central_panel(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.persistent);
    }

    fn persist_egui_memory(&self) -> bool {
        true
    }
}

fn settings_editor(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let mut changed = false;
    changed |= ui
        .add(egui::Slider::new(&mut settings.columns, 8..=1_000).text("Columns"))
        .changed();

    let mut exact = matches!(settings.row_sizing, RowSizing::Exact(_));
    if ui.checkbox(&mut exact, "Exact height").changed() {
        settings.row_sizing = if exact {
            RowSizing::Exact(60)
        } else {
            RowSizing::default()
        };
        changed = true;
    }
    match &mut settings.row_sizing {
        RowSizing::Auto {
            character_cell_ratio,
        } => {
            changed |= ui
                .add(
                    egui::Slider::new(character_cell_ratio, 0.25..=1.0).text("Cell width / height"),
                )
                .changed();
        }
        RowSizing::Exact(rows) => {
            changed |= ui
                .add(egui::Slider::new(rows, 1..=1_000).text("Rows"))
                .changed();
        }
    }

    ui.separator();
    ui.label("Character ramp (dark → light)");
    egui::ComboBox::from_id_salt(ui.id().with("built-in-ramp"))
        .selected_text(&settings.ramp.name)
        .show_ui(ui, |ui| {
            for ramp in CharacterRamp::built_ins() {
                if ui
                    .selectable_label(settings.ramp == ramp, &ramp.name)
                    .clicked()
                {
                    settings.ramp = ramp;
                    changed = true;
                }
            }
        });
    if ui
        .add(
            egui::TextEdit::singleline(&mut settings.ramp.characters).font(FontId::monospace(14.0)),
        )
        .changed()
    {
        settings.ramp.name = "Custom".to_owned();
        changed = true;
    }
    if let Err(error) = settings.ramp.validate() {
        ui.colored_label(Color32::LIGHT_RED, error.to_string());
    }
    changed |= ui
        .checkbox(&mut settings.invert_density, "Invert density")
        .changed();

    egui::ComboBox::from_id_salt(ui.id().with("dither"))
        .selected_text(match settings.dither {
            DitherMode::None => "No dithering",
            DitherMode::FloydSteinberg => "Floyd–Steinberg",
            DitherMode::Bayer4x4 => "Bayer 4×4",
        })
        .show_ui(ui, |ui| {
            changed |= ui
                .selectable_value(&mut settings.dither, DitherMode::None, "No dithering")
                .changed();
            changed |= ui
                .selectable_value(
                    &mut settings.dither,
                    DitherMode::FloydSteinberg,
                    "Floyd–Steinberg",
                )
                .changed();
            changed |= ui
                .selectable_value(&mut settings.dither, DitherMode::Bayer4x4, "Bayer 4×4")
                .changed();
        });

    ui.separator();
    ui.label("Tone and colour");
    changed |= ui
        .add(egui::Slider::new(&mut settings.brightness, -1.0..=1.0).text("Brightness"))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut settings.contrast, 0.0..=3.0).text("Contrast"))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut settings.gamma, 0.2..=3.0).text("Gamma"))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut settings.saturation, 0.0..=3.0).text("Saturation"))
        .changed();
    for (label, gain) in ["Red gain", "Green gain", "Blue gain"]
        .into_iter()
        .zip(settings.rgb_gain.iter_mut())
    {
        changed |= ui
            .add(egui::Slider::new(gain, 0.0..=3.0).text(label))
            .changed();
    }
    let mut matte = Color32::from_rgb(
        settings.transparency_matte[0],
        settings.transparency_matte[1],
        settings.transparency_matte[2],
    );
    ui.horizontal(|ui| {
        ui.label("Transparency matte");
        if ui.color_edit_button_srgba(&mut matte).changed() {
            settings.transparency_matte = [matte.r(), matte.g(), matte.b()];
            changed = true;
        }
    });
    changed
}

fn ascii_preview(
    ui: &mut egui::Ui,
    document: &AsciiDocument,
    color: bool,
    dark_background: bool,
    font_size: f32,
) {
    let background = if dark_background {
        Color32::from_gray(15)
    } else {
        Color32::from_gray(245)
    };
    egui::Frame::NONE.fill(background).show(ui, |ui| {
        let row_height = font_size * 1.2;
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, document.height as usize, |ui, rows| {
                for row_index in rows {
                    let Some(row) = document.row(row_index as u32) else {
                        continue;
                    };
                    let mut job = egui::text::LayoutJob::default();
                    job.wrap.max_width = f32::INFINITY;
                    if color {
                        for cell in row {
                            job.append(
                                &cell.character.to_string(),
                                0.0,
                                TextFormat {
                                    font_id: FontId::monospace(font_size),
                                    color: Color32::from_rgb(cell.rgb[0], cell.rgb[1], cell.rgb[2]),
                                    ..Default::default()
                                },
                            );
                        }
                    } else {
                        let text: String = row.iter().map(|cell| cell.character).collect();
                        job.append(
                            &text,
                            0.0,
                            TextFormat {
                                font_id: FontId::monospace(font_size),
                                color: if dark_background {
                                    Color32::WHITE
                                } else {
                                    Color32::BLACK
                                },
                                ..Default::default()
                            },
                        );
                    }
                    ui.label(job);
                }
            });
    });
}

fn normalized_to_screen(rect: egui::Rect, crop: CropRect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            rect.left() + crop.x * rect.width(),
            rect.top() + crop.y * rect.height(),
        ),
        egui::pos2(
            rect.left() + (crop.x + crop.width) * rect.width(),
            rect.top() + (crop.y + crop.height) * rect.height(),
        ),
    )
}

fn crop_drag_kind(crop: egui::Rect, pointer: egui::Pos2) -> CropDragKind {
    let candidates = [
        (crop.left_top(), CropDragKind::TopLeft),
        (crop.right_top(), CropDragKind::TopRight),
        (crop.left_bottom(), CropDragKind::BottomLeft),
        (crop.right_bottom(), CropDragKind::BottomRight),
    ];
    candidates
        .into_iter()
        .find(|(point, _)| point.distance(pointer) <= 14.0)
        .map(|(_, kind)| kind)
        .unwrap_or(CropDragKind::Move)
}

fn apply_crop_drag(crop: CropRect, kind: CropDragKind, delta: Vec2) -> CropRect {
    const MINIMUM: f32 = 0.01;
    match kind {
        CropDragKind::Move => CropRect {
            x: (crop.x + delta.x).clamp(0.0, 1.0 - crop.width),
            y: (crop.y + delta.y).clamp(0.0, 1.0 - crop.height),
            ..crop
        },
        CropDragKind::TopLeft => {
            let right = crop.x + crop.width;
            let bottom = crop.y + crop.height;
            let x = (crop.x + delta.x).clamp(0.0, right - MINIMUM);
            let y = (crop.y + delta.y).clamp(0.0, bottom - MINIMUM);
            CropRect {
                x,
                y,
                width: right - x,
                height: bottom - y,
            }
        }
        CropDragKind::TopRight => {
            let right = (crop.x + crop.width + delta.x).clamp(crop.x + MINIMUM, 1.0);
            let bottom = crop.y + crop.height;
            let y = (crop.y + delta.y).clamp(0.0, bottom - MINIMUM);
            CropRect {
                y,
                width: right - crop.x,
                height: bottom - y,
                ..crop
            }
        }
        CropDragKind::BottomLeft => {
            let right = crop.x + crop.width;
            let x = (crop.x + delta.x).clamp(0.0, right - MINIMUM);
            let bottom = (crop.y + crop.height + delta.y).clamp(crop.y + MINIMUM, 1.0);
            CropRect {
                x,
                width: right - x,
                height: bottom - crop.y,
                ..crop
            }
        }
        CropDragKind::BottomRight => {
            let right = (crop.x + crop.width + delta.x).clamp(crop.x + MINIMUM, 1.0);
            let bottom = (crop.y + crop.height + delta.y).clamp(crop.y + MINIMUM, 1.0);
            CropRect {
                width: right - crop.x,
                height: bottom - crop.y,
                ..crop
            }
        }
    }
}

fn selected_output_paths(path: PathBuf, formats: ExportFormats) -> BatchOutputPaths {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image_ascii.txt");
    let base = name
        .strip_suffix(".ansi.txt")
        .or_else(|| name.strip_suffix(".txt"))
        .unwrap_or(name);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    BatchOutputPaths {
        plain: formats.plain.then(|| parent.join(format!("{base}.txt"))),
        ansi: formats
            .ansi
            .then(|| parent.join(format!("{base}.ansi.txt"))),
    }
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff"
            )
        })
}

fn spawn_workers(context: egui::Context) -> (Sender<Job>, Receiver<JobResult>) {
    let (job_sender, job_receiver) = mpsc::channel::<Job>();
    let (result_sender, result_receiver) = mpsc::channel::<JobResult>();
    let job_receiver = Arc::new(Mutex::new(job_receiver));
    for worker_index in 0..2 {
        let jobs = job_receiver.clone();
        let results = result_sender.clone();
        let context = context.clone();
        let _ = thread::Builder::new()
            .name(format!("ascii-art-worker-{worker_index}"))
            .spawn(move || {
                loop {
                    let job = {
                        let receiver = jobs.lock().expect("worker queue poisoned");
                        receiver.recv()
                    };
                    let Ok(job) = job else {
                        break;
                    };
                    let result = match job {
                        Job::Load { id, path } => {
                            let result = decode_image(path)
                                .map(|image| {
                                    let preview = image::imageops::thumbnail(&image, 2_048, 2_048);
                                    LoadedImage {
                                        image: Arc::new(image),
                                        preview,
                                    }
                                })
                                .map_err(|error| error.to_string());
                            JobResult::Loaded { id, result }
                        }
                        Job::Convert {
                            id,
                            generation,
                            image,
                            crop,
                            settings,
                        } => JobResult::Converted {
                            id,
                            generation,
                            result: convert(&image, crop, &settings)
                                .map_err(|error| error.to_string()),
                        },
                        Job::Export(job) => {
                            let id = job.id;
                            JobResult::Exported {
                                id,
                                result: run_export(job),
                            }
                        }
                    };
                    if results.send(result).is_err() {
                        break;
                    }
                    context.request_repaint();
                }
            });
    }
    (job_sender, result_receiver)
}

fn run_export(job: ExportJob) -> Result<(), String> {
    if job.cancel.load(Ordering::Relaxed) {
        return Err("Export cancelled".to_owned());
    }
    let document =
        convert(&job.image, job.crop, &job.settings).map_err(|error| error.to_string())?;
    if let Some(path) = job.paths.plain {
        if job.cancel.load(Ordering::Relaxed) {
            return Err("Export cancelled".to_owned());
        }
        atomic_write(&path, render_plain(&document).as_bytes())
            .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    }
    if let Some(path) = job.paths.ansi {
        if job.cancel.load(Ordering::Relaxed) {
            return Err("Export cancelled".to_owned());
        }
        atomic_write(&path, render_ansi(&document).as_bytes())
            .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_move_is_clamped() {
        let crop = CropRect {
            x: 0.2,
            y: 0.2,
            width: 0.5,
            height: 0.5,
        };
        let moved = apply_crop_drag(crop, CropDragKind::Move, Vec2::new(1.0, -1.0));
        assert_eq!((moved.x, moved.y), (0.5, 0.0));
    }

    #[test]
    fn selected_paths_get_expected_suffixes() {
        let paths = selected_output_paths(
            PathBuf::from("art.txt"),
            ExportFormats {
                plain: true,
                ansi: true,
            },
        );
        assert_eq!(paths.plain, Some(PathBuf::from("art.txt")));
        assert_eq!(paths.ansi, Some(PathBuf::from("art.ansi.txt")));
    }
}
