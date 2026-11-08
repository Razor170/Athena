//! Draws the Intel Supervisor panel, following the approved mockup
//! (<https://claude.ai/artifact/AAU2CByJPT4bJj9xbcFqeY>).
//!
//! For now (PR 2 of 6): the Settings view has the "Intel channels" box from
//! the mockup. The Map view shows a plain list of the own pilots and the
//! latest reports, so the log reading can be tried out; the map and the
//! styled intel list replace it in the next pull requests.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};

use super::{IntelSupervisor, View};

/// The Settings view has the same width as the Mining panel.
const SETTINGS_WIDTH: f32 = 480.0;

// Colours from the mockup.
const YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x47);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const BLUE: Color32 = Color32::from_rgb(0x8f, 0xc3, 0xff);
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
    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.view, View::Map, "Map");
        ui.selectable_value(&mut module.view, View::Settings, "Settings");
    });
    for problem in module.problems.values() {
        ui.colored_label(YELLOW, small(problem));
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match module.view {
            View::Map => map_view(module, ui),
            View::Settings => settings_view(module, ui),
        });
}

/// Stand-in for the map: pilots and reports as plain text.
fn map_view(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
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

    ui.label(small("The map comes with the next update. Until then this lists what Athena reads from your chat logs.").color(DIM));

    ui.label(RichText::new("My pilots").color(BRIGHT).strong());
    let mut any = false;
    for pilot in module.shown_pilots() {
        any = true;
        ui.label(
            RichText::new(format!("{} · {}", pilot.name, system_name(pilot.system_id))).color(BLUE),
        );
    }
    if !any {
        ui.label(
            small("None yet. Pilots show up once they are logged in (Local chat).").color(DIM),
        );
    }

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
