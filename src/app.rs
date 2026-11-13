//! The main window: a bar of module toggle buttons on top and the dock area with all tabs below.

use eframe::egui;
use egui_dock::{DockArea, Style};

use crate::layout::{Layout, Tab};
use crate::modules::{self, Module, ModuleContext};
use crate::overlay::MiningOverlay;
use crate::settings::{FilePollRate, Settings};
use crate::storage::{DataDir, Saver};
use crate::updater::{StartupInfo, StripAction, Updater};
use crate::window_place::MainWindow;

/// How long the main window must stay put after a move or resize before its
/// new placement is saved, so dragging it around doesn't write the file over
/// and over.
const WINDOW_SAVE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

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
    /// The auto-updater: its strip under the toggle bar and its Settings section.
    updater: Updater,
    /// Set by "Restart now": start Athena again once this one has closed.
    restart_to_install: bool,
    /// The main window, to read where it is on screen.
    main_window: MainWindow,
    /// egui's context. A cheap handle (it is reference counted inside);
    /// kept for `on_exit`, which doesn't get one.
    egui_ctx: egui::Context,
    /// What the main window's position, size and maximized state were the
    /// last frame, to notice when the user moves or resizes it.
    window_seen: WindowSeen,
    /// When the window last moved or changed size, while that isn't saved yet.
    window_changed_at: Option<std::time::Instant>,
    /// Tabs the user closed with the tab's own close button this frame.
    /// Closed after the dock is drawn, through `Layout::close`, so their
    /// spot is remembered.
    tabs_to_close: Vec<Tab>,
}

/// The main window as egui last reported it: outer rectangle, maximized, minimized.
type WindowSeen = (Option<egui::Rect>, Option<bool>, Option<bool>);

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
        main_window: MainWindow,
        data_dir: DataDir,
        settings: Settings,
        settings_warning: Option<String>,
        saver: Saver,
        update_info: StartupInfo,
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
        let updater = Updater::start(egui_ctx, &settings, &data_dir, update_info);
        AthenaApp {
            updater,
            restart_to_install: false,
            mining_overlay: MiningOverlay::new(egui_ctx),
            modules,
            layout,
            settings,
            data_dir,
            saver,
            settings_panel,
            last_saved_layout: String::new(),
            main_window,
            egui_ctx: egui_ctx.clone(),
            window_seen: (None, None, None),
            window_changed_at: None,
            tabs_to_close: Vec::new(),
        }
    }

    /// Notices when the main window moved or changed size, and once it has
    /// been still for `WINDOW_SAVE_DELAY`, stores where it is in the layout
    /// (which then gets saved like any other layout change). Moving the
    /// window makes egui draw a frame, so this costs nothing while idle.
    fn track_window(&mut self, ctx: &egui::Context) {
        let seen = ctx.input(|i| {
            let viewport = i.viewport();
            (viewport.outer_rect, viewport.maximized, viewport.minimized)
        });
        if seen != self.window_seen {
            self.window_seen = seen;
            self.window_changed_at = Some(std::time::Instant::now());
            // One extra frame once the delay has passed, to save then.
            ctx.request_repaint_after(WINDOW_SAVE_DELAY);
            return;
        }
        if let Some(changed_at) = self.window_changed_at {
            let waited = changed_at.elapsed();
            if waited >= WINDOW_SAVE_DELAY {
                self.window_changed_at = None;
                self.remember_window_placement();
            } else {
                // egui forgets a delayed repaint once another frame is drawn,
                // so ask again for the time that is left.
                ctx.request_repaint_after(WINDOW_SAVE_DELAY - waited);
            }
        }
    }

    fn remember_window_placement(&mut self) {
        if let Some(placement) = self.main_window.read() {
            self.layout.window = Some(placement);
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

    /// The bar on top: one toggle button per module, Settings on the right.
    /// The buttons come from `self.modules`, so a new module gets one for free.
    fn toggle_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for module in &self.modules {
                let tab = Tab::Module(module.id().to_string());
                let badge = module.badge();
                toggle_button(ui, &mut self.layout, tab, module.title(), badge.as_deref());
            }
            // Right-to-left layout fills the rest of the row from the right edge.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle_button(ui, &mut self.layout, Tab::Settings, "Settings", None);
            });
        });
    }
}

/// Fill and text colours of a toggle button whose tab is open: brighter than
/// a closed one, so it shows at a glance which modules are open.
const OPEN_FILL: egui::Color32 = egui::Color32::from_rgb(0x5b, 0x6f, 0x86);
const OPEN_TEXT: egui::Color32 = egui::Color32::WHITE;
const CLOSED_FILL: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x2a, 0x2a);
const CLOSED_TEXT: egui::Color32 = egui::Color32::from_gray(0x8c);
const OPEN_EDGE: egui::Color32 = egui::Color32::from_rgb(0x8a, 0xa3, 0xbf);
const CLOSED_EDGE: egui::Color32 = egui::Color32::from_gray(0x3a);

/// The badge on a toggle button (for example unseen contract changes).
const BADGE_FILL: egui::Color32 = egui::Color32::from_rgb(0xe8, 0x91, 0x3a);
const BADGE_TEXT: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x1b, 0x1b);

/// A button in the top bar that opens or closes one tab. Bright when the tab
/// is open, dark when it's closed. `badge`, if any, is drawn after the title
/// on an orange background.
fn toggle_button(
    ui: &mut egui::Ui,
    layout: &mut Layout,
    tab: Tab,
    title: &str,
    badge: Option<&str>,
) {
    let open = layout.is_open(&tab);
    let (fill, text, edge) = if open {
        (OPEN_FILL, OPEN_TEXT, OPEN_EDGE)
    } else {
        (CLOSED_FILL, CLOSED_TEXT, CLOSED_EDGE)
    };
    // The title and the badge as one piece of text with two formats.
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat::simple(font.clone(), text);
    job.append(title, 0.0, plain);
    if let Some(badge) = badge {
        let format = egui::TextFormat {
            background: BADGE_FILL,
            ..egui::TextFormat::simple(font, BADGE_TEXT)
        };
        job.append(&format!(" {badge} "), 6.0, format);
    }
    let button = egui::Button::new(job).fill(fill);
    if crate::widgets::add_outlined(ui, true, button, egui::Stroke::new(1.0, edge)).clicked() {
        if open {
            layout.close(&tab, ui.ctx());
        } else {
            layout.open(tab);
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
        self.updater.update();
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

        egui::Panel::top("toggle_bar").show(ui, |ui| self.toggle_bar(ui));
        if self.updater.strip_visible() {
            match self.updater.strip_ui(ui) {
                StripAction::None => {}
                StripAction::OpenSettings => self.layout.open(Tab::Settings),
                StripAction::Restart => {
                    self.restart_to_install = true;
                    // Closes the window the normal way, so `on_exit` saves everything first.
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        let mut settings_changed = false;
        let mut viewer = TabViewer {
            modules: &mut self.modules,
            settings: &mut self.settings,
            data_dir: &self.data_dir,
            updater: &mut self.updater,
            panel: &mut self.settings_panel,
            settings_changed: &mut settings_changed,
            tabs_to_close: &mut self.tabs_to_close,
        };
        DockArea::new(&mut self.layout.dock)
            .style(Style::from_egui(ui.style()))
            // Hide the "close every tab in this group" button; one click
            // there is too easy to hit by accident.
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut viewer);
        for tab in std::mem::take(&mut self.tabs_to_close) {
            self.layout.close(&tab, ui.ctx());
        }
        self.track_window(ui.ctx());

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
        self.updater.frame_drawn();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings();
        self.remember_window_placement();
        self.layout.remember_floating_windows(&self.egui_ctx);
        self.save_layout_if_changed();
        for module in &mut self.modules {
            module.on_exit();
        }
        // Wait for the writes to finish before the process ends.
        self.saver.finish();
        if self.restart_to_install
            && let Err(error) = crate::updater::install::restart_to_install()
        {
            // Nothing is on screen any more; the update stays staged and
            // is installed on the next start instead.
            eprintln!("{error:#}");
        }
    }
}

/// Tells egui_dock how to draw each tab. It borrows the app's parts for one frame.
struct TabViewer<'a> {
    modules: &'a mut [Box<dyn Module>],
    settings: &'a mut Settings,
    data_dir: &'a DataDir,
    updater: &'a mut Updater,
    panel: &'a mut SettingsPanel,
    settings_changed: &'a mut bool,
    tabs_to_close: &'a mut Vec<Tab>,
}

impl egui_dock::TabViewer for TabViewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(&*tab)
    }

    /// The tab's own close button (or a middle click on the tab). egui_dock
    /// would remove the tab right away; instead we close it ourselves after
    /// the dock is drawn, so `Layout::close` can remember where it was.
    fn on_close(&mut self, tab: &mut Tab) -> egui_dock::tab_viewer::OnCloseResponse {
        if !self.tabs_to_close.contains(tab) {
            self.tabs_to_close.push(tab.clone());
        }
        egui_dock::tab_viewer::OnCloseResponse::Ignore
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
        if viewer.updater.settings_ui(ui, viewer.settings) {
            *viewer.settings_changed = true;
        }

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
