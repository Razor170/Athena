//! Draws the Intel Supervisor panel, following the approved mockup
//! (<https://claude.ai/artifact/AAU2CByJPT4bJj9xbcFqeY>).
//!
//! The toolbar (region picker, Follow, pilot chips, alert range, Mute),
//! the map with the alert banner and tags, a draggable split, and the
//! intel list with zKillboard links. Settings has the "Intel channels",
//! "Alerts", "Map" and "System tags" boxes.

use std::collections::HashMap;
use std::time::Duration;

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};

use super::alerts::{self, SystemMark};
use super::lookup;
use super::sound::{self, SoundKind};
use super::tags;
use super::{IntelSupervisor, Report, View, map, now};

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
const NOTE: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
const TEXT: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
const PILOT_NAME: Color32 = Color32::from_rgb(0x8f, 0xba, 0xff);
const SHIP_BORDER: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
const SHIP_TEXT: Color32 = Color32::from_rgb(0xe8, 0xe8, 0xe8);
// Report cards: new (red), faded, cleared, and past the fade time.
const NEW_FILL: Color32 = Color32::from_rgb(0x3a, 0x15, 0x12);
const FADED_FILL: Color32 = Color32::from_rgb(0x23, 0x21, 0x20);
const FADED_BORDER: Color32 = Color32::from_rgb(0x3a, 0x35, 0x33);
const CLEAR_FILL: Color32 = Color32::from_rgb(0x1c, 0x26, 0x1d);
const CLEAR_BORDER: Color32 = Color32::from_rgb(0x33, 0x50, 0x3a);
const OLD_FILL: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
// The alert banner.
const BANNER_TITLE: Color32 = Color32::from_rgb(0xff, 0xb3, 0xad);
const BANNER_TEXT: Color32 = Color32::from_rgb(0xf0, 0xd0, 0xcc);
/// While reports are fading, the map and list are redrawn this often (not
/// every frame): the badges count whole minutes.
const FADE_REPAINT: Duration = Duration::from_secs(30);

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

        ui.add_space(12.0);
        let alert_jumps = module.alert_jumps();
        ui.label(
            RichText::new(format!(
                "Alert at {alert_jumps} jumps or less · loud at {}",
                module.settings.loud_jumps.min(alert_jumps)
            ))
            .color(DIM),
        );
        let mute_text = match module.muted_until {
            Some(until) if module.is_muted() => {
                let left = (-now().seconds_since(until)).max(0);
                format!("Muted · {}:{:02}", left / 60, left % 60)
            }
            _ => "Mute 5 min".to_string(),
        };
        if ui
            .button(mute_text)
            .on_hover_text("No sound or flashing for 5 minutes. Click again to unmute.")
            .clicked()
        {
            module.toggle_mute();
        }
        let minimized = ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false));
        if module.is_muted() && !minimized {
            // Counts down once a second while muted and visible.
            ui.ctx().request_repaint_after(Duration::from_secs(1));
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
    let mut list_width = module.settings.list_width.clamp(
        MIN_LIST_WIDTH,
        (area.width() - MIN_MAP_WIDTH - handle_width).max(MIN_LIST_WIDTH),
    );
    // A very narrow tab: map and list share what there is.
    if area.width() < MIN_MAP_WIDTH + MIN_LIST_WIDTH + handle_width {
        list_width = ((area.width() - handle_width) / 2.0).max(0.0);
    }
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
    let now = now();
    let reports: Vec<&Report> = module.reports.iter().collect();
    let marks = alerts::system_marks(&reports, now, module.fade_seconds(), &module.distances);
    let shown_tags = tags::by_system(&module.tags, &module.settings.hidden_tag_groups);
    let mut map_ui = ui.new_child(egui::UiBuilder::new().max_rect(map_rect));
    let action = module.map.show(
        &mut map_ui,
        &map::MapContent {
            pilots: &pilots,
            marks: &marks,
            alert_frame: module.alert.is_some() && module.settings.flash,
            fade_minutes: module.settings.fade_minutes,
            tags: &shown_tags,
            activity: &module.activity,
        },
    );
    drop(shown_tags);
    if let Some(action) = action {
        module.map_action(action);
    }
    alert_banner(module, ui, map_rect);
    tag_dialog(module, ui);
    schedule_fade_repaint(module, ui, &marks);

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

/// The red banner across the top of the map while an alert is showing.
fn alert_banner(module: &mut IntelSupervisor, ui: &mut egui::Ui, map_rect: egui::Rect) {
    let Some(alert) = &module.alert else {
        return;
    };
    let universe = module.universe();
    let system = universe
        .and_then(|u| u.system(alert.system_id))
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let pilots = and_list(&alert.pilots);
    let title = match alert.jumps {
        0 => format!("{system} · in system with {pilots}"),
        n => format!("{system} · {} from {pilots}", alerts::jumps_text(n)),
    };
    let mut details: Vec<String> = alert.report.message.ships.clone();
    details.extend(
        lookup::pilots_to_show(&alert.report.message.pilot_candidates, &module.characters)
            .into_iter()
            .map(|(name, _)| name),
    );
    details.push(format!("reported by {}", alert.report.sender));
    details.push(clock_text(alert.report.time.unix_seconds));

    let area = egui::Rect::from_min_size(
        map_rect.min + Vec2::splat(12.0),
        Vec2::new((map_rect.width() - 24.0).max(0.0), 34.0),
    );
    let mut dismissed = false;
    let mut banner_ui = ui.new_child(egui::UiBuilder::new().max_rect(area));
    banner_ui.set_clip_rect(area.expand(2.0));
    egui::Frame::new()
        .fill(NEW_FILL)
        .stroke(Stroke::new(2.0, alerts::RED))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(&mut banner_ui, |ui| {
            ui.set_width(ui.available_width());
            // Dismiss first, on the right, so long text is cut short
            // instead of pushing it out of the map.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                dismissed = ui.button(small("Dismiss")).clicked();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(title).color(BANNER_TITLE).strong())
                            .truncate(),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(details.join(" · ")).color(BANNER_TEXT))
                            .truncate(),
                    );
                });
            });
        });
    if dismissed {
        module.alert = None;
    }
}

/// "Pilot Alpha", "Pilot Alpha and Pilot Bravo", "A, B and C".
fn and_list(names: &[String]) -> String {
    match names {
        [] => "your pilots".to_string(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Reports fade by age, so the map needs a redraw now and then while any
/// is still fading, but never while Athena is minimized.
fn schedule_fade_repaint(
    module: &IntelSupervisor,
    ui: &egui::Ui,
    marks: &HashMap<i64, SystemMark>,
) {
    let minimized = ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false));
    if !minimized && (!marks.is_empty() || module.alert.is_some()) {
        ui.ctx().request_repaint_after(FADE_REPAINT);
    }
}

/// A small coloured chip with white text, like the "1 jump" badge.
fn badge(ui: &mut egui::Ui, text: &str, fill: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(small(text).color(Color32::WHITE).strong());
        });
}

/// The intel list: one card per report, newest first.
fn report_list(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    let Some(data) = module.static_data.clone() else {
        return;
    };
    let universe = &data.universe;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Intel").color(BRIGHT).strong().size(13.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .selectable_label(module.settings.only_in_range, small("Only in range"))
                .on_hover_text("Show only reports within the alert range.")
                .clicked()
            {
                module.settings.only_in_range = !module.settings.only_in_range;
                module.save_settings();
            }
            let count = module.settings.channels.len();
            let channels = match count {
                1 => "1 channel".to_string(),
                n => format!("{n} channels"),
            };
            ui.label(small(channels).color(DIM));
            // Green while the logs can be read, yellow if not.
            let ok = !module.problems.contains_key("chatlogs") && count > 0;
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(dot.center(), 4.0, if ok { alerts::GREEN } else { YELLOW });
        });
    });
    if module.settings.channels.is_empty() {
        ui.label(small("Pick your intel channels in Settings.").color(DIM));
    }

    let now = now();
    let fade = module.fade_seconds();
    let alert_jumps = module.alert_jumps();
    // A clicked pilot name; opened after the loop, which borrows the reports.
    let mut open_link: Option<String> = None;
    for report in &module.reports {
        let jumps = alerts::report_jumps(report, &module.distances);
        if module.settings.only_in_range && jumps.is_none_or(|j| j > alert_jumps) {
            continue;
        }
        let fraction = alerts::fade_fraction(report, now, fade);
        let named = !report.message.systems.is_empty();
        let (fill, border, badge_color) = if !named || fraction >= 1.0 {
            (OLD_FILL, BORDER, alerts::OLD_GREY)
        } else if report.message.clear {
            (CLEAR_FILL, CLEAR_BORDER, alerts::GREEN)
        } else {
            (
                alerts::mix(NEW_FILL, FADED_FILL, fraction),
                alerts::mix(alerts::RED, FADED_BORDER, fraction),
                alerts::fade_color(fraction),
            )
        };
        let systems: Vec<String> = report
            .message
            .systems
            .iter()
            .filter_map(|&id| universe.system(id).map(|s| s.name.clone()))
            .collect();
        egui::Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(5))
            .inner_margin(egui::Margin::symmetric(7, 5))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 3.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [52.0, 14.0],
                        egui::Label::new(
                            RichText::new(clock_text(report.time.unix_seconds))
                                .color(DIM)
                                .monospace()
                                .size(11.0),
                        ),
                    );
                    if named {
                        ui.label(RichText::new(systems.join(", ")).color(BRIGHT).strong());
                    }
                    if let Some(jumps) = jumps {
                        badge(ui, &alerts::jumps_text(jumps), badge_color);
                    }
                    if report.message.clear {
                        badge(ui, "clear", badge_color);
                    }
                    for ship in &report.message.ships {
                        egui::Frame::new()
                            .stroke(Stroke::new(1.0, SHIP_BORDER))
                            .corner_radius(CornerRadius::same(3))
                            .inner_margin(egui::Margin::symmetric(5, 0))
                            .show(ui, |ui| {
                                ui.label(small(ship).color(SHIP_TEXT));
                            });
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(small(&report.channel).color(NOTE));
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(58.0);
                    // Guesses ESI confirmed as characters: zKillboard links.
                    for (name, id) in
                        lookup::pilots_to_show(&report.message.pilot_candidates, &module.characters)
                    {
                        let link = ui
                            .add(
                                egui::Label::new(RichText::new(&name).color(PILOT_NAME).strong())
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Open on zKillboard");
                        if link.clicked() {
                            open_link = Some(lookup::zkillboard_url(id));
                        }
                    }
                    let color = if named { TEXT } else { DIM };
                    ui.label(RichText::new(&report.text).color(color))
                        .on_hover_text(format!("{} > {}", report.sender, report.text));
                });
            });
    }
    if let Some(url) = open_link {
        match crate::shell::open_url(&url) {
            Ok(()) => {
                module.problems.remove("browser");
            }
            Err(problem) => {
                module.problems.insert("browser", problem);
            }
        }
    }
    ui.add_space(6.0);
    ui.label(
        small(format!(
            "Pilot names open zKillboard in your browser. Reports fade from red to grey over {} minutes; \"{}\" turns the system green.",
            module.settings.fade_minutes,
            module.settings.clear_words.join("\" or \"")
        ))
        .color(NOTE),
    );
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

    alerts_settings(module, ui);

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
        let mut tint = module.settings.esi_tint;
        if ui
            .checkbox(&mut tint, "Tint systems by ESI kills and jumps (last hour)")
            .changed()
        {
            module.set_esi_tint(tint);
        }
        ui.label(
            small(
                "The tint uses ESI's public data, refreshed at most once an hour. Off by default. \
                 Purple glow: player kills; grey-blue glow: jumps. Hover a system for the numbers.",
            )
            .color(DIM),
        );
    });

    tags_settings(module, ui);
}

/// The "System tags" box in Settings: Import, Export, the list of tags,
/// and which groups show on the map.
fn tags_settings(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    let mut import = false;
    let mut export = false;
    let mut remove: Option<usize> = None;
    let mut group_toggled: Option<(String, bool)> = None;
    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("System tags ({})", module.tags.len()))
                    .color(BRIGHT)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                export = ui
                    .button("Export")
                    .on_hover_text("Copies all tags to the clipboard.")
                    .clicked();
                import = ui
                    .button("Import")
                    .on_hover_text("Adds the tags on the clipboard.")
                    .clicked();
            });
        });
        ui.label(
            small(
                "Same text format as RIFT (System;Label;Colour;Icon;…;Group), so you can paste \
                 your RIFT export straight in. Import reads the clipboard; export copies to it. \
                 Right-click a system on the map to add a tag.",
            )
            .color(DIM),
        );
        if let Some(message) = &module.tag_message {
            ui.label(small(message).color(YELLOW));
        }
        if module.tags.is_empty() {
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("intel_tags")
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("intel_tag_list")
                    .num_columns(5)
                    .spacing([10.0, 3.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (index, tag) in module.tags.iter().enumerate() {
                            ui.label(RichText::new(&tag.system_name).monospace().size(11.0));
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                            tags::paint_icon(
                                ui.painter(),
                                rect.center(),
                                11.0,
                                &tag.icon,
                                tag.color,
                            );
                            ui.label(&tag.label);
                            ui.label(small(&tag.group).color(DIM));
                            if ui
                                .small_button("×")
                                .on_hover_text("Remove this tag")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.horizontal_wrapped(|ui| {
            ui.label(small("Show groups on map:").color(DIM));
            for group in tags::groups(&module.tags) {
                let mut shown = !module.settings.hidden_tag_groups.contains(&group);
                let name = if group.is_empty() { "No group" } else { &group };
                if ui.checkbox(&mut shown, small(name)).changed() {
                    group_toggled = Some((group.clone(), shown));
                }
            }
        });
    });
    if import {
        module.import_tags();
    }
    if export {
        module.export_tags();
    }
    if let Some(index) = remove {
        module.remove_tag(index);
    }
    if let Some((group, shown)) = group_toggled {
        module.set_tag_group_shown(&group, shown);
    }
}

/// The "Add tag…" window, while it is open: label, group, colour, icon.
fn tag_dialog(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    let Some(system_id) = module.tag_dialog.as_ref().map(|d| d.system_id) else {
        return;
    };
    let system = module
        .universe()
        .and_then(|u| u.system(system_id))
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let groups = tags::groups(&module.tags);
    let existing_tags = module.tags.clone();
    let Some(dialog) = module.tag_dialog.as_mut() else {
        return;
    };
    let mut add = false;
    let mut cancel = false;
    egui::Window::new(format!("Add tag · {system}"))
        .id(egui::Id::new("intel_add_tag"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            egui::Grid::new("intel_add_tag_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Label");
                    let label = ui.text_edit_singleline(&mut dialog.label);
                    // Ready to type as soon as the window opens.
                    if dialog.focus_label {
                        label.request_focus();
                        dialog.focus_label = false;
                    }
                    ui.end_row();

                    ui.label("Group");
                    if dialog.new_group {
                        ui.text_edit_singleline(&mut dialog.group);
                    } else {
                        let mut picked = None;
                        egui::ComboBox::from_id_salt("intel_tag_group")
                            .selected_text(dialog.group.clone())
                            .show_ui(ui, |ui| {
                                for group in &groups {
                                    if ui.selectable_label(&dialog.group == group, group).clicked()
                                    {
                                        picked = Some(group.clone());
                                    }
                                }
                                if ui.selectable_label(false, "New group…").clicked() {
                                    dialog.new_group = true;
                                    dialog.group.clear();
                                }
                            });
                        // A picked group brings its colour along.
                        if let Some(group) = picked {
                            if let Some(color) = tags::group_color(&existing_tags, &group) {
                                dialog.color = color;
                            }
                            dialog.group = group;
                        }
                    }
                    ui.end_row();

                    ui.label("Colour");
                    ui.horizontal(|ui| {
                        ui.color_edit_button_srgba(&mut dialog.color);
                        ui.label(small(tags::color_text(dialog.color)).color(DIM));
                    });
                    ui.end_row();

                    ui.label("Icon");
                    ui.horizontal(|ui| {
                        for (icon, name) in tags::ICONS {
                            let selected = dialog.icon == icon;
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::new(30.0, 24.0), egui::Sense::click());
                            let fill = if selected { CHIP_FILL } else { BOX_FILL };
                            let stroke = if selected { CHIP_BORDER } else { BORDER };
                            ui.painter().rect(
                                rect,
                                CornerRadius::same(4),
                                fill,
                                Stroke::new(1.0, stroke),
                                egui::StrokeKind::Inside,
                            );
                            tags::paint_icon(ui.painter(), rect.center(), 14.0, icon, dialog.color);
                            if response.on_hover_text(name).clicked() {
                                dialog.icon = icon.to_string();
                            }
                        }
                    });
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label(small("Enter adds the tag, Esc cancels.").color(DIM));
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = !dialog.label.trim().is_empty();
                    add = ui.add_enabled(ready, egui::Button::new("Add")).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            let (enter, escape) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if enter && !dialog.label.trim().is_empty() {
                add = true;
            }
            if escape {
                cancel = true;
            }
        });
    if add {
        module.add_tag_from_dialog();
    } else if cancel {
        module.tag_dialog = None;
    }
}

/// Saves the settings once a number field is done changing: when a drag
/// ends or typing finishes, not every frame of a drag.
fn save_when_done(module: &mut IntelSupervisor, response: &egui::Response) {
    let done = response.drag_stopped()
        || response.lost_focus()
        || (response.changed() && !response.dragged() && !response.has_focus());
    if done {
        module.save_settings();
    }
}

/// The "Alerts" box in Settings.
fn alerts_settings(module: &mut IntelSupervisor, ui: &mut egui::Ui) {
    let mut play: Option<u32> = None;
    let mut open_folder = false;
    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Alerts").color(BRIGHT).strong());
        egui::Grid::new("intel_alert_settings")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Alert when a report is this close");
                let response = ui.add(
                    egui::DragValue::new(&mut module.settings.alert_jumps)
                        .range(0..=sound::MAX_JUMPS)
                        .suffix(" jumps"),
                );
                save_when_done(module, &response);
                ui.end_row();

                ui.label("Loud alert at");
                let alert_jumps = module.settings.alert_jumps;
                let response = ui
                    .add(
                        egui::DragValue::new(&mut module.settings.loud_jumps)
                            .range(0..=alert_jumps)
                            .suffix(" jumps"),
                    )
                    .on_hover_text("Full volume, played twice.");
                save_when_done(module, &response);
                ui.end_row();

                ui.label("Sound");
                let before = module.settings.sound;
                egui::ComboBox::from_id_salt("intel_sound")
                    .selected_text(match module.settings.sound {
                        SoundKind::Voice => "Voice",
                        SoundKind::Beep => "Beep",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut module.settings.sound, SoundKind::Voice, "Voice");
                        ui.selectable_value(&mut module.settings.sound, SoundKind::Beep, "Beep");
                    });
                if module.settings.sound != before {
                    module.save_settings();
                }
                ui.end_row();
            });

        ui.horizontal_wrapped(|ui| {
            for jumps in 0..=module.settings.alert_jumps.min(sound::MAX_JUMPS) {
                let text = match jumps {
                    0 => "▶ Enemy in system".to_string(),
                    1 => "▶ 1 jump".to_string(),
                    n => format!("▶ {n}"),
                };
                if ui
                    .button(text)
                    .on_hover_text(sound::spoken_text(jumps))
                    .clicked()
                {
                    play = Some(jumps);
                }
            }
            open_folder = ui.button("Open sounds folder").clicked();
        });
        ui.label(
            small(
                "\"Enemy 3 jumps\" and so on, one WAV per distance in the sounds folder. \
                 Drop in your own file with the same name to replace one.",
            )
            .color(DIM),
        );

        egui::Grid::new("intel_alert_settings_2")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Volume");
                let mut percent = (module.settings.volume * 100.0).round();
                let response = ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix(" %"));
                if response.changed() {
                    module.settings.volume = percent / 100.0;
                }
                save_when_done(module, &response);
                ui.end_row();

                ui.label("Reports fade to grey after");
                let response = ui.add(
                    egui::DragValue::new(&mut module.settings.fade_minutes)
                        .range(1..=60)
                        .suffix(" min"),
                );
                save_when_done(module, &response);
                ui.end_row();

                ui.label("Words that mean clear");
                let response = ui.text_edit_singleline(&mut module.clear_words_text);
                if response.lost_focus() {
                    let text = module.clear_words_text.clone();
                    module.set_clear_words(&text);
                    module.clear_words_text = module.settings.clear_words.join(", ");
                }
                ui.end_row();
            });
        if ui
            .checkbox(
                &mut module.settings.flash,
                "Flash the map and the taskbar button",
            )
            .changed()
        {
            module.save_settings();
        }
        ui.label(
            small("Distance counts from the nearest of your pilots, by gate jumps.").color(DIM),
        );
    });
    if let Some(jumps) = play {
        module.play_sample(jumps);
    }
    if open_folder {
        match sound::open_sounds_folder() {
            Ok(()) => {
                module.problems.remove("sounds");
            }
            Err(problem) => {
                module.problems.insert("sounds", problem);
            }
        }
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
