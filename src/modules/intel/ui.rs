//! Draws the Intel Supervisor panel, following the approved mockup
//! (<https://claude.ai/artifact/AAU2CByJPT4bJj9xbcFqeY>).
//!
//! For now (PR 3 of 6): the toolbar (region picker, Follow, pilot chips),
//! the map, a draggable split, and a plain list of reports where the
//! mockup's intel list goes in the next pull request. Settings has the
//! "Intel channels" box and the Map box's Follow switch.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};

use super::{IntelSupervisor, View, map};

/// The Settings view has the same width as the Mining panel.
const SETTINGS_WIDTH: f32 = 480.0;

// Colours from the mockup.
const YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x47);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const CHIP_FILL: Color32 = Color32::from_rgb(0x1d, 0x33, 0x52);
const CHIP_BORDER: Color32 = Color32::from_rgb(0x3d, 0x6f, 0xae);
const CHIP_TEXT: Color32 = Color32::from_rgb(0xcf, 0xe2, 0xff);
const HANDLE: Color32 = Color32::from_rgb(0x4a, 0x4f, 0x57);

/// The map and the list can't be dragged narrower than this.
const MIN_MAP_WIDTH: f32 = 300.0;
const MIN_LIST_WIDTH: f32 = 220.0;
const BOX_FILL: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
const BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(11.0)
}

fn section_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BOX_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
}

/// The panel.
pub fn show(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    toolbar(module, ui);
    for problem in module.problems.values() {
        ui.colored_label(YELLOW, small(problem));
    }
    match module.view {
        View::Map => map_and_list(module, ui),
        View::Settings => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| settings_view(module, ui));
        }
    }
}

/// Map/Settings, the region picker, Follow and the pilot chips.
fn toolbar(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut module.view, View::Map, "Map");
        ui.selectable_value(&mut module.view, View::Settings, "Settings");
        if module.view != View::Map {
            return;
        }
        let Some(data) = module.static_data.clone() else {
            return;
        };
        let universe = &data.universe;
        ui.add_space(12.0);

        ui.label(RichText::new("Region").color(DIM));
        let shown = module.map.region_id();
        let shown_name = shown
            .and_then(|id| universe.region(id))
            .map(|r| r.name.clone())
            .unwrap_or_default();
        let mut picked = None;
        egui::ComboBox::from_id_salt("intel_region")
            .selected_text(shown_name)
            .height(400.0)
            .show_ui(ui, |ui| {
                for region in universe.regions_by_name() {
                    if ui
                        .selectable_label(shown == Some(region.id), &region.name)
                        .clicked()
                    {
                        picked = Some(region.id);
                    }
                }
            });
        if let Some(region) = picked {
            module.show_region(region);
        }

        let pilots: Vec<(String, i64)> = module
            .shown_pilots()
            .map(|p| (p.name.clone(), p.system_id))
            .collect();
        let active = module.active_pilot().map(|p| p.name.clone());
        let follow_text = match &active {
            Some(name) => format!("Follow: {name}"),
            None => "Follow: nobody yet".to_string(),
        };
        ui.menu_button(follow_text, |ui| {
            if ui
                .selectable_label(module.follow.is_none(), "Whoever jumped last")
                .clicked()
            {
                module.follow = None;
                ui.close();
            }
            for (name, _) in &pilots {
                if ui
                    .selectable_label(module.follow.as_ref() == Some(name), name)
                    .clicked()
                {
                    module.follow = Some(name.clone());
                    ui.close();
                }
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new("My pilots").color(DIM));
        let mut clicked = None;
        let all: Vec<(String, i64)> = module
            .current_pilots()
            .map(|p| (p.name.clone(), p.system_id))
            .collect();
        for (name, system_id) in all {
            let hidden = module.settings.hidden_pilots.contains(&name);
            let system = universe
                .system(system_id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if pilot_chip(ui, &format!("{name} · {system}"), hidden)
                .on_hover_text(if hidden {
                    "Hidden: not on the map and not counted for alerts. Click to show."
                } else {
                    "Click to hide this pilot from the map and from alerts."
                })
                .clicked()
            {
                clicked = Some(name);
            }
        }
        if let Some(name) = clicked {
            module.toggle_pilot_hidden(&name);
        }
    });
}

/// A rounded chip like the mockup's "Pilot Alpha · KV-8SN".
fn pilot_chip(ui: &mut egui::Ui, text: &str, hidden: bool) -> egui::Response {
    let (fill, stroke, color) = if hidden {
        (BOX_FILL, BORDER, DIM)
    } else {
        (CHIP_FILL, CHIP_BORDER, CHIP_TEXT)
    };
    ui.add(
        egui::Button::new(RichText::new(text).color(color))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(10)),
    )
}

/// The map on the left, the report list on the right, and a handle
/// between them to drag the split.
fn map_and_list(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    let area = ui.available_rect_before_wrap();
    let handle_width = 12.0;
    let list_width = module.settings.list_width.clamp(
        MIN_LIST_WIDTH,
        (area.width() - MIN_MAP_WIDTH - handle_width).max(MIN_LIST_WIDTH),
    );
    let map_rect = egui::Rect::from_min_max(
        area.min,
        egui::pos2(area.right() - list_width - handle_width, area.bottom()),
    );
    let handle_rect = egui::Rect::from_min_max(
        egui::pos2(map_rect.right(), area.top()),
        egui::pos2(map_rect.right() + handle_width, area.bottom()),
    );
    let list_rect = egui::Rect::from_min_max(egui::pos2(handle_rect.right(), area.top()), area.max);

    let pilots = module.pilots_by_system();
    let mut map_ui = ui.new_child(egui::UiBuilder::new().max_rect(map_rect));
    module
        .map
        .show(&mut map_ui, &map::MapContent { pilots: &pilots });

    // The handle: a short bar in the middle, dragged sideways.
    let handle = ui
        .interact(
            handle_rect,
            ui.id().with("intel_split"),
            egui::Sense::drag(),
        )
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let bar = egui::Rect::from_center_size(handle_rect.center(), Vec2::new(4.0, 48.0));
    ui.painter().rect_filled(bar, CornerRadius::same(2), HANDLE);
    if handle.dragged() {
        module.settings.list_width = list_width - handle.drag_delta().x;
    }
    if handle.drag_stopped() {
        module.save_settings();
    }

    let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
    section_frame().show(&mut list_ui, |ui| {
        ui.set_min_size(ui.available_size());
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| report_list(module, ui));
    });
    ui.allocate_rect(area, egui::Sense::hover());
}

/// Stand-in for the intel list: reports as plain text.
fn report_list(module: &IntelSupervisor, ui: &mut egui::Ui) {
    let Some(data) = module.static_data.clone() else {
        return;
    };
    let universe = &data.universe;
    let system_name = |id: i64| {
        universe
            .system(id)
            .map(|s| s.name.clone())
            .unwrap_or_default()
    };
    ui.label(RichText::new("Intel").color(BRIGHT).strong());
    if module.settings.channels.is_empty() {
        ui.label(small("Pick your intel channels in Settings.").color(DIM));
    }
    for report in &module.reports {
        let systems: Vec<String> = report
            .message
            .systems
            .iter()
            .map(|&id| system_name(id))
            .collect();
        let mut parts = vec![clock_text(report.time.unix_seconds)];
        parts.push(if systems.is_empty() {
            "–".to_string()
        } else {
            systems.join(", ")
        });
        if !report.message.ships.is_empty() {
            parts.push(report.message.ships.join(", "));
        }
        if report.message.clear {
            parts.push("clear".to_string());
        }
        parts.push(format!("{} > {}", report.sender, report.text));
        parts.push(report.channel.clone());
        let color = if systems.is_empty() { DIM } else { BRIGHT };
        ui.label(small(parts.join(" · ")).color(color));
    }
}

fn settings_view(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    ui.set_max_width(SETTINGS_WIDTH);
    let mut toggled: Option<(String, bool)> = None;
    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Intel channels").color(BRIGHT).strong());
        ui.label(small("Found in your Chatlogs folder. Tick the ones to watch.").color(DIM));
        // Ticked channels that weren't found (no recent log) stay listed,
        // so they can be unticked.
        let mut names: Vec<String> = module.channels_found.clone();
        for channel in &module.settings.channels {
            if !names.contains(channel) {
                names.push(channel.clone());
            }
        }
        names.sort();
        if names.is_empty() {
            ui.label(
                small("No chat logs found yet. Check the log folder in Athena's settings.")
                    .color(DIM),
            );
        }
        for name in names {
            let mut watched = module.settings.channels.contains(&name);
            if ui.checkbox(&mut watched, &name).changed() {
                toggled = Some((name, watched));
            }
        }
    });
    // Changed after drawing, because the loop above borrows the settings.
    if let Some((name, watched)) = toggled {
        module.set_channel_watched(&name, watched);
    }

    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Map").color(BRIGHT).strong());
        if ui
            .checkbox(
                &mut module.settings.follow_regions,
                "Follow the active pilot into other regions",
            )
            .changed()
        {
            module.save_settings();
        }
        ui.label(
            small(
                "The active pilot is the one picked on the Follow button, or whoever jumped last.",
            )
            .color(DIM),
        );
    });
}

/// "14:52:10" (EVE time) from unix seconds.
fn clock_text(unix: i64) -> String {
    let seconds = unix.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}
