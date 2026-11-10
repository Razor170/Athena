//! The main window: a menu bar on top and the dock area with all tabs below.

use eframe::egui;
use egui_dock::{DockArea, Style};

use crate::layout::{Layout, Tab};
use crate::modules::{self, Module, ModuleContext};
use crate::overlay::MiningOverlay;
use crate::settings::{FilePollRate, Settings};
use crate::storage::{DataDir, Saver};

pub struct AthenaApp {
    modules: Vec<Box<dyn Module>>,
    layout: Layout,
    settings: Settings,
    data_dir: DataDir,
    saver: Saver,
    settings_panel: SettingsPanel,
    /// The Mining Supervisor's hold bars overlay.
    mining_overlay: MiningOverlay,
    /// The layout text we last handed to the saver, so we only save when it changed.
    last_saved_layout: String,
}

/// State of the settings tab that isn't a setting itself.
struct SettingsPanel {
    /// The log folder text while the user is typing. Applied when the field loses focus.
    log_folder_text: String,
    /// Problems to show the user, such as a settings or layout file that couldn't be read.
    warnings: Vec<String>,
    /// How many times the window was drawn. Lets us check that Athena doesn't
    /// redraw while idle: the number should only go up when something happens.
    frames_drawn: u64,
    /// CPU time of the last frame in seconds, as measured by eframe.
    last_frame_cpu: Option<f32>,
}

impl AthenaApp {
    pub fn new(
        egui_ctx: &egui::Context,
        data_dir: DataDir,
        settings: Settings,
        settings_warning: Option<String>,
        saver: Saver,
    ) -> AthenaApp {
        // Set before the modules start their log watchers.
        crate::settings::set_file_poll_rate(settings.file_poll_rate);
        let modules = modules::create_all(egui_ctx, &settings, &data_dir.path);
        let module_ids: Vec<String> = modules.iter().map(|m| m.id().to_string()).collect();
        // Reading the layout file here is fine: this runs once before the window shows.
        let (layout, layout_warning) = Layout::load(&data_dir.path, &module_ids);
        // `into_iter().flatten()` keeps only the warnings that are `Some`.
        let warnings = [settings_warning, layout_warning]
            .into_iter()
            .flatten()
            .collect();
        let settings_panel = SettingsPanel {
            log_folder_text: settings.log_folder.display().to_string(),
            warnings,
            frames_drawn: 0,
            last_frame_cpu: None,
        };
        AthenaApp {
            mining_overlay: MiningOverlay::new(egui_ctx),
            modules,
            layout,
            settings,
            data_dir,
            saver,
            settings_panel,
            last_saved_layout: String::new(),
        }
    }

    fn save_settings(&self) {
        if let Ok(text) = self.settings.to_toml() {
            self.saver
                .save(Settings::file_path(&self.data_dir.path), text);
        }
    }

    /// Saves the layout if it differs from what we saved last time.
    fn save_layout_if_changed(&mut self) {
        let Ok(text) = self.layout.to_toml() else {
            return;
        };
        if text != self.last_saved_layout {
            self.saver
                .save(Layout::file_path(&self.data_dir.path), text.clone());
            self.last_saved_layout = text;
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("View", |ui| {
                for module in &self.modules {
                    let tab = Tab::Module(module.id().to_string());
                    toggle_tab(ui, &mut self.layout, tab, module.title());
                }
                ui.separator();
                toggle_tab(ui, &mut self.layout, Tab::Settings, "Settings");
            });
        });
    }
}

/// A checkbox in the View menu that opens or closes one tab.
fn toggle_tab(ui: &mut egui::Ui, layout: &mut Layout, tab: Tab, title: &str) {
    let mut open = layout.is_open(&tab);
    if ui.checkbox(&mut open, title).changed() {
        if open {
            layout.open(tab);
        } else {
            layout.close(&tab);
        }
    }
}

impl eframe::App for AthenaApp {
    // Runs before every frame, and also while the window is minimized when a
    // background thread called `request_repaint`. Modules drain their channels here.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let module_ctx = ModuleContext {
            settings: &self.settings,
        };
        for module in &mut self.modules {
            module.update(&module_ctx);
        }
        // Here rather than in `ui`, because `logic` also runs while the main
        // window is minimized, and the overlay must keep up then.
        if let Some(place) = self.mining_overlay.take_moved() {
            for module in &mut self.modules {
                module.bars_overlay_moved(place);
            }
        }
        let bars = self.modules.iter().find_map(|m| m.bars_overlay());
        self.mining_overlay.show(bars);
    }

    /// The colour every window is cleared to before drawing. eframe uses one
    /// colour for all windows. It is fully transparent and every window
    /// paints its own background (the main window does it in `ui`).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.settings_panel.frames_drawn += 1;
        self.settings_panel.last_frame_cpu = frame.info().cpu_usage;
        // The clear colour is transparent (see `clear_color`), so fill the
        // main window first.
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));

        let mut settings_changed = false;
        let mut viewer = TabViewer {
            modules: &mut self.modules,
            settings: &mut self.settings,
            data_dir: &self.data_dir,
            panel: &mut self.settings_panel,
            settings_changed: &mut settings_changed,
        };
        DockArea::new(&mut self.layout.dock)
            .style(Style::from_egui(ui.style()))
            // Hide the "close every tab in this group" button; one click
            // there is too easy to hit by accident.
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut viewer);

        if settings_changed {
            self.save_settings();
            for module in &mut self.modules {
                module.settings_changed(&self.settings);
            }
        }

        // Save the layout as soon as it changes, so a crash or a Windows
        // shutdown doesn't lose it. `ui` only runs when something happened,
        // so this costs nothing while idle. Skip it while a mouse button is
        // held, so dragging a tab doesn't save on every frame.
        if !ui.input(|i| i.pointer.any_down()) {
            self.save_layout_if_changed();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings();
        self.save_layout_if_changed();
        for module in &mut self.modules {
            module.on_exit();
        }
        // Wait for the writes to finish before the process ends.
        self.saver.finish();
    }
}

/// Tells egui_dock how to draw each tab. It borrows the app's parts for one frame.
struct TabViewer<'a> {
    modules: &'a mut [Box<dyn Module>],
    settings: &'a mut Settings,
    data_dir: &'a DataDir,
    panel: &'a mut SettingsPanel,
    settings_changed: &'a mut bool,
}

impl egui_dock::TabViewer for TabViewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(&*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Module(id) => match self.modules.iter().find(|m| m.id() == id) {
                Some(module) => module.title().into(),
                None => id.as_str().into(),
            },
            Tab::Settings => "Settings".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Module(id) => {
                let ctx = ModuleContext {
                    settings: self.settings,
                };
                match self.modules.iter_mut().find(|m| m.id() == id) {
                    Some(module) => module.ui(ui, &ctx),
                    None => {
                        ui.label(format!("Unknown module: {id}"));
                    }
                }
            }
            Tab::Settings => settings_ui(ui, self),
        }
    }
}

fn settings_ui(ui: &mut egui::Ui, viewer: &mut TabViewer) {
    let panel = &mut *viewer.panel;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for warning in &panel.warnings {
            ui.colored_label(ui.visuals().warn_fg_color, warning);
            ui.add_space(8.0);
        }

        ui.heading("EVE logs");
        ui.label("The folder that contains Chatlogs and Gamelogs.");
        let response = ui.add(
            egui::TextEdit::singleline(&mut panel.log_folder_text).desired_width(f32::INFINITY),
        );
        if response.lost_focus() {
            let new_folder = std::path::PathBuf::from(panel.log_folder_text.trim());
            if new_folder != viewer.settings.log_folder {
                viewer.settings.log_folder = new_folder;
                *viewer.settings_changed = true;
            }
        }
        if ui.button("Reset to default").clicked() {
            viewer.settings.log_folder = Settings::default().log_folder;
            panel.log_folder_text = viewer.settings.log_folder.display().to_string();
            *viewer.settings_changed = true;
        }

        ui.add_space(8.0);
        ui.label("Check log files for new lines every");
        // `horizontal_wrapped` puts the choices in one row and wraps them
        // to a second row if the panel is narrow.
        ui.horizontal_wrapped(|ui| {
            let current = viewer.settings.file_poll_rate;
            for rate in FilePollRate::ALL {
                ui.radio_value(&mut viewer.settings.file_poll_rate, rate, rate.label());
            }
            if viewer.settings.file_poll_rate != current {
                // The watcher threads read this before each sleep, so
                // nothing needs to restart.
                crate::settings::set_file_poll_rate(viewer.settings.file_poll_rate);
                *viewer.settings_changed = true;
            }
        });
        ui.label(
            egui::RichText::new(
                "Used by every module that reads EVE logs. Shorter shows intel and mining changes sooner; longer uses a little less CPU. Default 250 ms.",
            )
            .weak(),
        );

        ui.add_space(12.0);
        ui.heading("Updates");
        ui.label(format!("This version: {}", crate::version::display_text()));

        ui.add_space(12.0);
        ui.heading("Data folder");
        ui.label(viewer.data_dir.path.display().to_string());
        if viewer.data_dir.is_fallback {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "The folder next to Athena.exe isn't writable, so settings are stored in %APPDATA%\\Athena instead.",
            );
        }

        ui.add_space(12.0);
        ui.heading("Diagnostics");
        ui.label(format!("Renderer: {}", renderer_name()));
        ui.label(format!(
            "Display scale: {:.0}%",
            ui.ctx().pixels_per_point() * 100.0
        ));
        ui.label(format!("Frames drawn: {}", panel.frames_drawn));
        if let Some(cpu) = panel.last_frame_cpu {
            ui.label(format!("Last frame CPU time: {:.2} ms", cpu * 1000.0));
        }

    });
}

fn renderer_name() -> &'static str {
    if cfg!(feature = "wgpu") {
        "wgpu"
    } else {
        "glow (OpenGL)"
    }
}
