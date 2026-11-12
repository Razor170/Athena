//! Draws the Mining Supervisor panel: the Live, History and Settings views
//! from the mockup Kevin approved
//! (<https://claude.ai/artifact/AeFsPvZpRzPkk5rUdNcrfZ>).
//!
//! Everything here only reads the module's state and records clicks; the
//! work behind a click (ending a session, saving settings) is done by the
//! methods in `mod.rs`.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Vec2};

use super::config::OverlaySettings;
use super::gamelog::LogTime;
use super::prices::{self, PriceSide};
use super::scan::{self, BeltScan};
use super::session::{CharacterTracker, GuardState};
use super::store::{PeriodTotal, SessionKind, StoredSession, Totals};
use super::{AlertLevel, HistoryMode, MiningSupervisor, View};
use crate::localtime::civil_from_days;
use crate::overlay::mining_bars::HoldBar;

/// The panel is designed for a quarter of a 1080p screen. With more than one
/// column of cards on the Live view, each column is at most this wide.
const MAX_WIDTH: f32 = 480.0;
/// Card columns may shrink to this so that four fit on a 1080p screen
/// (4 x 480 px plus gaps is a bit more than 1920). Below it a column is
/// dropped instead.
const MIN_COLUMN_WIDTH: f32 = 400.0;
/// Space between card columns.
const COLUMN_GAP: f32 = 6.0;
/// Width of the name column on the cards and in the alert list.
const NAME_WIDTH: f32 = 92.0;

// Colours from the mockup.
const GREEN: Color32 = Color32::from_rgb(0x5c, 0xbf, 0x6a);
const YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x47);
const RED: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
pub(super) const PURPLE: Color32 = Color32::from_rgb(0xa3, 0x7e, 0xf0);
const LIGHT_PURPLE: Color32 = Color32::from_rgb(0xc5, 0xa8, 0xff);
const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const SOFT: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const BOX_FILL: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
const TILE_FILL: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
const BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
const BAR_BACK: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);

/// Small text (11 px in the mockup).
fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(11.0)
}

/// The panel.
pub fn show(module: &mut MiningSupervisor, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Only the Live view grows with the column setting; History and
            // Settings stay one column wide.
            let columns = live_columns(module.settings.columns(), ui.available_width());
            if module.view == View::Live {
                ui.set_max_width(columns.total_width());
            } else {
                ui.set_max_width(MAX_WIDTH);
            }
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            view_tabs(module, ui);
            for problem in module.problems.values() {
                ui.colored_label(YELLOW, small(problem));
            }
            match module.view {
                View::Live => live_view(module, ui, columns),
                View::History => history_view(module, ui),
                View::Settings => settings_view(module, ui),
            }
        });
}

fn view_tabs(module: &mut MiningSupervisor, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.view, View::Live, "Live");
        ui.selectable_value(&mut module.view, View::History, "History");
        ui.selectable_value(&mut module.view, View::Settings, "Settings");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if crate::overlay::is_supported() {
                let mut on = module.settings.overlay.enabled;
                if ui
                    .toggle_value(&mut on, small("Overlay"))
                    .on_hover_text("Show the hold bars overlay on top of the screen.")
                    .changed()
                {
                    module.settings.overlay.enabled = on;
                    if !on {
                        module.overlay_move_mode = false;
                    }
                    module.settings_edited();
                }
            }
            if module.view == View::Live {
                let text = match &module.prices {
                    Some(prices) => {
                        let age = prices.age_secs(now_unix());
                        format!("Jita prices {} old", duration_text(age.max(0)))
                    }
                    None => "Waiting for Jita prices".to_string(),
                };
                ui.label(small(text).color(DIM));
            }
        });
    });
}

/// How the Live view's character cards are laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Columns {
    count: usize,
    /// Width of one column in pixels.
    width: f32,
}

impl Columns {
    /// All columns plus the gaps between them.
    fn total_width(self) -> f32 {
        let n = self.count as f32;
        n * self.width + (n - 1.0) * COLUMN_GAP
    }
}

/// The column setting fitted into the tab: columns are up to 480 px wide and
/// shrink to 400 px; when even that doesn't fit, a column is dropped. Always
/// at least one column.
fn live_columns(setting: usize, available: f32) -> Columns {
    let mut count = setting.max(1);
    loop {
        let n = count as f32;
        let width = ((available - (n - 1.0) * COLUMN_GAP) / n).min(MAX_WIDTH);
        if count == 1 || width >= MIN_COLUMN_WIDTH {
            return Columns {
                count,
                width: width.max(0.0),
            };
        }
        count -= 1;
    }
}

// ---------------------------------------------------------------------------
// Live view

/// A rounded box with a 1 px border, like the mockup's sections.
fn section_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BOX_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
}

fn live_view(module: &mut MiningSupervisor, ui: &mut egui::Ui, columns: Columns) {
    let now = LogTime {
        unix_seconds: now_unix(),
    };
    summary_bar(module, ui, now);

    // Cards for the characters that are shown and have something to show,
    // in the order set in Settings.
    let names: Vec<String> = module
        .settings
        .ordered(module.trackers.keys())
        .into_iter()
        .filter(|name| module.settings.character(name).show)
        .filter(|name| {
            module
                .trackers
                .get(name)
                .is_some_and(|t| t.session().is_some() || !t.hold_units().is_empty())
        })
        .collect();
    if names.is_empty() {
        ui.label(
            small(if module.caught_up {
                "No mining yet. Characters show up here with their first mining cycle."
            } else {
                "Reading today's game logs..."
            })
            .color(DIM),
        );
    }
    let column_width = columns.width;
    let mut emptied = None;
    ui.spacing_mut().item_spacing = Vec2::new(COLUMN_GAP, 4.0);
    // Cards fill a row left to right, then the next row, in Settings order.
    for row in names.chunks(columns.count) {
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            for name in row {
                let Some(tracker) = module.trackers.get(name) else {
                    continue;
                };
                // A fixed-width box for each card, so the columns line up.
                ui.allocate_ui_with_layout(
                    Vec2::new(column_width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(column_width);
                        if character_card(module, tracker, ui, now) {
                            emptied = Some(name.clone());
                        }
                    },
                );
            }
        });
    }
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    if let Some(name) = emptied {
        module.mark_emptied(&name);
    }

    if columns.count == 1 {
        alert_list(module, ui);
        belt_scan(module, ui);
    } else {
        // Belt scan on the left (one column wide), alert log next to it.
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(column_width, 0.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(column_width);
                    belt_scan(module, ui);
                },
            );
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 0.0),
                Layout::top_down(Align::Min),
                |ui| alert_list(module, ui),
            );
        });
    }
    legend(ui);
}

/// Two lines. Top: session ISK, m³ mined, session time and End session.
/// Bottom: ISK/h, m³/h, m³/s and Empty all. Buttons on the right.
fn summary_bar(module: &mut MiningSupervisor, ui: &mut egui::Ui, now: LogTime) {
    let mut isk = 0.0;
    let mut m3 = 0.0;
    let mut started: Option<LogTime> = None;
    for tracker in module.trackers.values() {
        let Some(session) = tracker.session() else {
            continue;
        };
        isk += session_isk(module, &session.mined_units);
        m3 += session_m3(module, &session.mined_units);
        started = Some(started.map_or(session.started, |s| s.min(session.started)));
    }
    let seconds = started.map_or(0, |s| now.seconds_since(s).max(0));
    let hours = seconds as f64 / 3600.0;

    let mut end_clicked = false;
    let mut empty_all_clicked = false;
    section_frame()
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let per_hour = |amount: f64| if hours > 0.01 { amount / hours } else { 0.0 };
            ui.horizontal(|ui| {
                ui.label(RichText::new("Session").color(DIM));
                ui.label(RichText::new(isk_text(isk)).color(BRIGHT).strong());
                ui.add_space(6.0);
                ui.label(RichText::new("Mined").color(DIM));
                ui.label(
                    RichText::new(format!("{} m³", short_volume(m3)))
                        .color(BRIGHT)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let button = egui::Button::new(
                        small("End session").color(Color32::from_rgb(0xf0, 0xd0, 0xa0)),
                    )
                    .fill(Color32::from_rgb(0x3a, 0x2a, 0x1a))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(0x6b, 0x4a, 0x24)));
                    let hint = if module.can_value_sessions() {
                        "Ends the session for every pilot and saves it to History, valued at the current Jita prices."
                    } else {
                        "Jita prices or ore data haven't loaded yet, so the session can't be valued. It keeps running until they are in."
                    };
                    if ui
                        .add_enabled(started.is_some(), button)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        end_clicked = true;
                    }
                    if started.is_some() {
                        ui.label(RichText::new(duration_text(seconds)).color(DIM));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("ISK/h").color(DIM));
                ui.label(RichText::new(isk_text(per_hour(isk))).color(BRIGHT).strong());
                ui.add_space(6.0);
                ui.label(RichText::new("m³/h").color(DIM));
                let m3_hour = per_hour(m3);
                ui.label(RichText::new(short_volume(m3_hour)).color(BRIGHT).strong());
                ui.label(small(format!("{} m³/s", per_second_text(m3_hour / 3600.0))).color(SOFT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // A hold EVE called full counts too, even if Athena saw
                    // none of its ore (e.g. loaded before Athena ran).
                    let anything_in_holds = module
                        .trackers
                        .values()
                        .any(|t| !t.hold_units().is_empty() || t.hold_reported_full());
                    if ui
                        .add_enabled(anything_in_holds, egui::Button::new(small("Empty all")))
                        .on_hover_text(
                            "Click after unloading everyone at a station: empties every ore hold at once.",
                        )
                        .clicked()
                    {
                        empty_all_clicked = true;
                    }
                });
            });
        });
    if end_clicked {
        module.end_session();
    }
    if empty_all_clicked {
        module.mark_all_emptied();
    }
}

/// The volume of a session's ore, in m³ (unknown ores count as 0).
fn session_m3(module: &MiningSupervisor, units: &std::collections::BTreeMap<String, u64>) -> f64 {
    units
        .iter()
        .map(|(ore, &count)| module.volumes.get(ore).unwrap_or(0.0) * count as f64)
        .sum()
}

/// The session's value with the current prices, 0 until prices load.
fn session_isk(module: &MiningSupervisor, units: &std::collections::BTreeMap<String, u64>) -> f64 {
    match (&module.static_data, &module.prices) {
        (Some(data), Some(prices)) => {
            prices::value_units(
                units,
                data,
                prices,
                module.settings.price_side,
                module.settings.yields(),
            )
            .isk
        }
        _ => 0.0,
    }
}

/// One character card. Returns true when "Emptied" was clicked.
fn character_card(
    module: &MiningSupervisor,
    tracker: &CharacterTracker,
    ui: &mut egui::Ui,
    now: LogTime,
) -> bool {
    let fill = tracker.hold_fill(&module.volumes);
    let capacity = tracker.settings.hold_capacity_m3.unwrap_or(0.0);
    let alert_at = tracker.settings.fill_alert_percent;
    let percent = tracker.fill_percent(&module.volumes).unwrap_or(0.0);
    let bar_color = fill_color(tracker, percent);

    // What the character is doing, and how the border looks.
    let quiet_secs = tracker
        .session()
        .map(|s| now.seconds_since(s.last_cycle).max(0));
    let idle = quiet_secs.is_some_and(|q| q >= tracker.settings.idle_after_secs);
    let guard = tracker.guard_state();
    let session_isk = tracker
        .session()
        .map_or(0.0, |s| session_isk(module, &s.mined_units));
    let mined_secs = tracker
        .session()
        .map_or(0, |s| s.last_cycle.seconds_since(s.started));
    let session_m3 = tracker
        .session()
        .map_or(0.0, |s| session_m3(module, &s.mined_units));
    // The character's own rate over the time it was mining (first to last
    // cycle), so it doesn't sink while the pilot is idle or docked.
    let rate = match m3_per_second(session_m3, mined_secs) {
        Some(rate) => format!(" · {} m³/s", per_second_text(rate)),
        None => String::new(),
    };
    let mut stats = format!(
        "{}{rate} · {}",
        duration_text(mined_secs),
        isk_text(session_isk)
    );
    let (status, status_color, border) = if percent >= tracker.settings.full_percent {
        let border = if guard == GuardState::Stopped {
            Border::Dashed(RED)
        } else {
            Border::Solid(BORDER)
        };
        ("Hold full".to_string(), RED, border)
    } else if idle {
        let quiet = duration_text(quiet_secs.unwrap_or(0));
        // Still dashed red when a miner reported stopping (e.g. rock
        // depleted) and nothing was mined since.
        let border = if tracker.stopped_by_log() {
            Border::Dashed(RED)
        } else {
            Border::Solid(BORDER)
        };
        (format!("Idle {quiet}"), SOFT, border)
    } else if guard == GuardState::Stopped {
        ("Miner stopped".to_string(), RED, Border::Dashed(RED))
    } else if let GuardState::CrystalBroken { rate_percent } = guard {
        if let Some(rate) = rate_percent {
            stats = format!("{rate:.0}% of normal rate · {}", isk_text(session_isk));
        }
        (
            "Crystal broke".to_string(),
            LIGHT_PURPLE,
            Border::Dashed(PURPLE),
        )
    } else if tracker.session().is_some() {
        ("Mining".to_string(), GREEN, Border::Solid(BORDER))
    } else {
        ("Not mining".to_string(), SOFT, Border::Solid(BORDER))
    };

    let mut emptied = false;
    let frame = egui::Frame::new()
        .fill(BOX_FILL)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(7, 4))
        .stroke(match border {
            Border::Solid(color) => Stroke::new(2.0, color),
            // The dashed border is drawn on top afterwards.
            Border::Dashed(_) => Stroke::new(2.0, Color32::TRANSPARENT),
        });
    let response = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 3.0);
        ui.horizontal(|ui| {
            name_label(ui, &tracker.character, BRIGHT, true);
            // Emptied button and percentage on the right, bar in between.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let button = egui::Button::new(small("Emptied"));
                if ui
                    .add(button)
                    .on_hover_text(
                        "Click after unloading at a station. EVE writes nothing to the log when you unload.",
                    )
                    .clicked()
                {
                    emptied = true;
                }
                ui.add_sized(
                    [34.0, 14.0],
                    egui::Label::new(
                        RichText::new(format!("{percent:.0}%"))
                            .color(bar_color)
                            .strong(),
                    ),
                );
                let width = ui.available_width();
                // When EVE says the hold is full but our count is lower
                // (ore from before Athena ran, or a hold size set too big),
                // stretch the bar so it looks full too.
                let bar_capacity = bar_capacity(capacity, percent, fill.total_m3());
                hold_bar(
                    ui,
                    width,
                    bar_capacity,
                    fill.compressed_m3,
                    fill.uncompressed_m3,
                    bar_color,
                    alert_at,
                );
            });
        });
        ui.horizontal(|ui| {
            name_label(ui, &status, status_color, false);
            ui.label(small(&stats).color(SOFT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    small(format!(
                        "C {} · U {} m³",
                        thousands(fill.compressed_m3),
                        thousands(fill.uncompressed_m3)
                    ))
                    .color(SOFT),
                );
            });
        });
        if !fill.unknown_items.is_empty() {
            ui.label(
                small(format!("Unknown volume: {}", fill.unknown_items.join(", "))).color(YELLOW),
            );
        }
    });
    if let Border::Dashed(color) = border {
        dashed_rect(ui, response.response.rect, color);
    }
    emptied
}

enum Border {
    Solid(Color32),
    Dashed(Color32),
}

/// A label of fixed width that cuts long names off.
fn name_label(ui: &mut egui::Ui, text: &str, color: Color32, strong: bool) {
    let mut rich = RichText::new(text).color(color);
    rich = if strong {
        rich.strong()
    } else {
        rich.size(11.0)
    };
    // A fixed-size box with a left-to-right layout keeps the name on the left.
    ui.allocate_ui_with_layout(
        Vec2::new(NAME_WIDTH, 14.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(NAME_WIDTH);
            ui.add(egui::Label::new(rich).truncate());
        },
    );
}

/// Green, yellow from the alert level, red when full.
fn fill_color(tracker: &CharacterTracker, percent: f64) -> Color32 {
    if percent >= tracker.settings.full_percent {
        RED
    } else if percent >= tracker.settings.fill_alert_percent {
        YELLOW
    } else {
        GREEN
    }
}

/// The m³ the bar's full width stands for. When EVE says the hold is full
/// but our count is lower (ore from before Athena ran, or a hold size set
/// too big), the bar is stretched so it looks full too.
fn bar_capacity(capacity: f64, percent: f64, total_m3: f64) -> f64 {
    if percent >= 100.0 && total_m3 > 0.0 {
        capacity.min(total_m3)
    } else {
        capacity
    }
}

/// A character's bar for the hold bars overlay: the same parts and colours
/// as the bar on the Live view.
pub(super) fn overlay_bar(module: &MiningSupervisor, tracker: &CharacterTracker) -> HoldBar {
    let fill = tracker.hold_fill(&module.volumes);
    let percent = tracker.fill_percent(&module.volumes).unwrap_or(0.0);
    let capacity = bar_capacity(
        tracker.settings.hold_capacity_m3.unwrap_or(0.0),
        percent,
        fill.total_m3(),
    );
    let share = |m3: f64| {
        if capacity > 0.0 {
            (m3 / capacity).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    };
    HoldBar {
        character: tracker.character.clone(),
        compressed: share(fill.compressed_m3),
        uncompressed: share(fill.uncompressed_m3),
        color: fill_color(tracker, percent),
        alert_mark: (capacity > 0.0)
            .then(|| (tracker.settings.fill_alert_percent / 100.0).clamp(0.0, 1.0) as f32),
        percent: tracker
            .fill_percent(&module.volumes)
            .map(|p| p.clamp(0.0, 999.0).round() as u32),
    }
}

/// The ore hold bar: compressed ore (purple), then the rest, plus a white
/// mark at the alert level.
fn hold_bar(
    ui: &mut egui::Ui,
    width: f32,
    capacity: f64,
    compressed: f64,
    uncompressed: f64,
    color: Color32,
    alert_percent: f64,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(40.0), 10.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(3), BAR_BACK);
    if capacity > 0.0 {
        let share = |m3: f64| (m3 / capacity).clamp(0.0, 1.0) as f32 * rect.width();
        let compressed_w = share(compressed);
        let uncompressed_w = share(uncompressed).min(rect.width() - compressed_w);
        let mut x = rect.left();
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(compressed_w, 10.0)),
            CornerRadius::ZERO,
            PURPLE,
        );
        x += compressed_w;
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(uncompressed_w, 10.0)),
            CornerRadius::ZERO,
            color,
        );
        let mark = rect.left() + (alert_percent / 100.0).clamp(0.0, 1.0) as f32 * rect.width();
        painter.line_segment(
            [
                egui::pos2(mark, rect.top()),
                egui::pos2(mark, rect.bottom()),
            ],
            Stroke::new(2.0, BRIGHT),
        );
    }
}

/// A dashed 2 px border around `rect`.
fn dashed_rect(ui: &egui::Ui, rect: egui::Rect, color: Color32) {
    let r = rect.shrink(1.0);
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    let shapes = egui::Shape::dashed_line(&corners, Stroke::new(2.0, color), 6.0, 4.0);
    ui.painter().extend(shapes);
}

fn alert_list(module: &MiningSupervisor, ui: &mut egui::Ui) {
    if module.alerts.is_empty() {
        return;
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    let now = now_unix();
    for alert in &module.alerts {
        let color = match alert.level {
            AlertLevel::Error => RED,
            AlertLevel::Warning => YELLOW,
            AlertLevel::Crystal => LIGHT_PURPLE,
            AlertLevel::Info => SOFT,
        };
        ui.horizontal(|ui| {
            ui.add_sized(
                [34.0, 14.0],
                egui::Label::new(small(clock_text(alert.time.unix_seconds)).color(DIM)),
            );
            name_label(ui, &alert.character, TEXT, false);
            ui.label(small(&alert.text).color(color));
            ui.label(small(ago_text(now - alert.time.unix_seconds)).color(DIM));
        });
    }
    ui.spacing_mut().item_spacing.y = 6.0;
}

fn belt_scan(module: &mut MiningSupervisor, ui: &mut egui::Ui) {
    let mut new_belt = false;
    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Belt scan").color(BRIGHT).strong());
            if let Some(latest) = module.belt_scans.latest() {
                let age = now_unix() - latest.taken.unix_seconds;
                ui.label(small(format!("Last scan {}", ago_text(age))).color(SOFT));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("New belt").clicked() {
                    new_belt = true;
                }
                let (dot, text) = if super::clipboard::is_supported() {
                    (GREEN, "Watching clipboard")
                } else {
                    (DIM, "Clipboard watching needs Windows")
                };
                ui.label(small(text).color(SOFT));
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, dot);
            });
        });

        let Some(latest) = module.belt_scans.latest() else {
            ui.label(
                small("Copy a Mining Survey Scanner result in EVE (Ctrl+A, Ctrl+C) and it shows up here. Copy another one later to see how fast the belt is being mined.")
                    .color(DIM),
            );
            return;
        };
        // The rate is rolling: the latest scan is compared with the newest
        // scan at least a minute older. Until there is one, the latest scan is
        // compared with itself, which gives no rate.
        let previous = module.belt_scans.previous();
        let progress = scan::belt_progress(previous.unwrap_or(latest), latest);
        let value_left = scan_value(module, latest);

        let gap = 6.0;
        let tile_width = (ui.available_width() - 2.0 * gap) / 3.0;
        ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
        let compared = previous.is_some();
        ui.horizontal(|ui| {
            let since = if compared {
                format!("−{} since last scan", short_m3(progress.m3_mined))
            } else {
                "first scan".to_string()
            };
            tile(ui, tile_width, "m³ left", &short_m3(progress.m3_left), &since);
            let gone = if compared {
                format!("{} gone since last scan", progress.rocks_gone)
            } else {
                String::new()
            };
            tile(
                ui,
                tile_width,
                "Rocks left",
                &progress.rocks_left.to_string(),
                &gone,
            );
            let value = value_left.map_or("?".to_string(), isk_text);
            tile(ui, tile_width, "Value left", &value, "reprocessed, Jita");
        });
        ui.horizontal(|ui| {
            let (rate, rate_note, cleared, at) =
                match (progress.m3_per_hour, progress.seconds_to_clear) {
                    (Some(rate), Some(seconds)) => (
                        format!("{} m³/h", short_m3(rate)),
                        // m³/s goes on the small line: the tile is too narrow for both.
                        format!("{} m³/s · whole belt", per_second_text(rate / 3600.0)),
                        format!("about {}", duration_text(seconds)),
                        format!(
                            "around {} EVE time at this rate",
                            clock_text(latest.taken.unix_seconds + seconds)
                        ),
                    ),
                    _ => (
                        "–".to_string(),
                        "everyone on the belt".to_string(),
                        "–".to_string(),
                        "copy another scan in a few minutes".to_string(),
                    ),
                };
            tile(ui, tile_width, "Belt rate", &rate, &rate_note);
            tile(ui, tile_width * 2.0 + gap, "Belt cleared in", &cleared, &at);
        });

        family_table(module, ui, latest);

        let footnote = match previous {
            Some(previous) => format!(
                "Previous scan at {}, latest at {} ({} apart). Copy a new survey scan in EVE (Ctrl+A, Ctrl+C) and it is picked up automatically; the rate always uses the latest scan and the one before it.",
                clock_text(previous.taken.unix_seconds),
                clock_text(latest.taken.unix_seconds),
                duration_text(latest.taken.seconds_since(previous.taken))
            ),
            None => format!(
                "Scan at {}. Copy a new survey scan in EVE (Ctrl+A, Ctrl+C) and it is picked up automatically.",
                clock_text(latest.taken.unix_seconds)
            ),
        };
        ui.label(small(footnote).color(DIM));
    });
    if new_belt {
        module.belt_scans.clear();
    }
}

/// A stat tile: small label, big value, small note.
fn tile(ui: &mut egui::Ui, width: f32, label: &str, value: &str, note: &str) {
    egui::Frame::new()
        .fill(TILE_FILL)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.vertical(|ui| {
                ui.label(small(label).color(DIM));
                ui.label(RichText::new(value).size(15.0).color(BRIGHT).strong());
                ui.label(small(note).color(DIM));
            });
        });
}

/// The reprocessed value of everything left in a scanned belt.
fn scan_value(module: &MiningSupervisor, scan: &BeltScan) -> Option<f64> {
    let (Some(data), Some(prices)) = (&module.static_data, &module.prices) else {
        return None;
    };
    let mut units = std::collections::BTreeMap::new();
    for rock in &scan.rocks {
        *units.entry(rock.ore.clone()).or_insert(0) += rock.units;
    }
    Some(
        prices::value_units(
            &units,
            data,
            prices,
            module.settings.price_side,
            module.settings.yields(),
        )
        .isk,
    )
}

/// Ore families in the scan: rocks, m³ and ISK.
fn family_table(module: &MiningSupervisor, ui: &mut egui::Ui, latest: &BeltScan) {
    egui::Grid::new("belt_families")
        .num_columns(4)
        .spacing([8.0, 2.0])
        .min_col_width(44.0)
        .show(ui, |ui| {
            ui.label(small("Ore (all grades)").color(DIM));
            ui.label(small("Rocks").color(DIM));
            ui.label(small("m³").color(DIM));
            ui.label(small("ISK").color(DIM));
            ui.end_row();
            for family in latest.by_family() {
                let rocks: Vec<&scan::Rock> = latest
                    .rocks
                    .iter()
                    .filter(|r| scan::ore_family(&r.ore) == family.family)
                    .collect();
                let isk = match (&module.static_data, &module.prices) {
                    (Some(data), Some(prices)) => {
                        let value: Option<f64> = rocks
                            .iter()
                            .map(|r| {
                                prices::unit_value(
                                    &r.ore,
                                    data,
                                    prices,
                                    module.settings.price_side,
                                    module.settings.yields(),
                                )
                                .map(|v| v * r.units as f64)
                            })
                            .sum();
                        value.map_or("?".to_string(), isk_text)
                    }
                    _ => "?".to_string(),
                };
                ui.label(small(&family.family));
                ui.label(small(family.rocks.to_string()));
                ui.label(small(short_m3(family.volume_m3)));
                ui.label(small(isk));
                ui.end_row();
            }
        });
}

fn legend(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        for (color, text) in [
            (PURPLE, "Compressed"),
            (GREEN, "Ore"),
            (YELLOW, "Over alert level"),
            (RED, "Full"),
        ] {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 8.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(2), color);
            ui.label(small(text).color(SOFT));
        }
    });
}

// ---------------------------------------------------------------------------
// History view

fn history_view(module: &mut MiningSupervisor, ui: &mut egui::Ui) {
    let history = &module.history;
    let gap = 6.0;
    let width = (ui.available_width() - 3.0 * gap) / 4.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (label, totals) in [
            ("Today", history.today),
            ("Yesterday", history.yesterday),
            ("This week", history.this_week),
            ("Last week", history.last_week),
        ] {
            total_tile(ui, width, label, totals);
        }
    });

    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.history_mode, HistoryMode::Sessions, "Sessions");
        ui.selectable_value(&mut module.history_mode, HistoryMode::Daily, "Daily");
        ui.selectable_value(&mut module.history_mode, HistoryMode::Weekly, "Weekly");
        if module.history_mode == HistoryMode::Sessions {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(small("Click a session for pilot details").color(DIM));
            });
        }
    });

    match module.history_mode {
        HistoryMode::Sessions => session_list(module, ui),
        HistoryMode::Daily => period_list(ui, &module.history.days, false),
        HistoryMode::Weekly => period_list(ui, &module.history.weeks, true),
    }
    ui.label(
        small("A session groups all pilots mining at the same time. ISK is saved with the Jita prices when the session ends and never recalculated. Daily and Weekly group sessions by EVE day or week (Monday start).")
            .color(Color32::from_rgb(0x8a, 0x8a, 0x8a)),
    );
}

fn total_tile(ui: &mut egui::Ui, width: f32, label: &str, totals: Totals) {
    section_frame()
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(width - 18.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.vertical(|ui| {
                ui.label(small(label).color(DIM));
                ui.label(
                    RichText::new(isk_text(totals.isk))
                        .size(14.0)
                        .color(BRIGHT)
                        .strong(),
                );
                ui.label(small(format!("{} m³", compact(totals.m3))).color(DIM));
            });
        });
}

fn session_list(module: &mut MiningSupervisor, ui: &mut egui::Ui) {
    if module.history.sessions.is_empty() {
        ui.label(small("No finished sessions yet. Click \"End session\" on the Live view when you are done mining.").color(DIM));
        return;
    }
    let mut toggled = None;
    section_frame()
        .inner_margin(egui::Margin::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for session in &module.history.sessions {
                let open = module.open_session == Some(session.id);
                if session_row(ui, session, open) {
                    toggled = Some(session.id);
                }
                if open {
                    pilot_details(ui, session);
                }
                ui.separator();
            }
        });
    if let Some(id) = toggled {
        module.open_session = if module.open_session == Some(id) {
            None
        } else {
            Some(id)
        };
    }
}

/// One session row; returns true when clicked.
fn session_row(ui: &mut egui::Ui, session: &StoredSession, open: bool) -> bool {
    let pilots = session.pilots.len();
    let title = if pilots == 1 {
        "1 pilot".to_string()
    } else {
        format!("{pilots} pilots")
    };
    let seconds = session.ended_unix - session.started_unix;
    let hours = seconds as f64 / 3600.0;
    let when = format!(
        "{} · {}-{} · {}",
        date_text(session.started_unix),
        clock_text(session.started_unix),
        clock_text(session.ended_unix),
        duration_text(seconds)
    );
    let kind = match session.kind {
        SessionKind::Ice => "Ice · ",
        SessionKind::Moon => "Moon · ",
        SessionKind::Gas => "Gas · ",
        SessionKind::Mixed | SessionKind::Ore => "",
    };
    let per_hour = if hours > 0.01 {
        format!(" · {}/h", isk_text(session.isk() / hours))
    } else {
        String::new()
    };
    let detail = format!("{kind}{} m³{per_hour}", thousands(session.m3()));

    let fill = if open {
        TILE_FILL
    } else {
        Color32::TRANSPARENT
    };
    let response = egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 2.0);
            ui.horizontal(|ui| {
                open_arrow(ui, open);
                ui.label(RichText::new(title).color(BRIGHT).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(isk_text(session.isk()))
                            .color(BRIGHT)
                            .strong(),
                    );
                });
            });
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(when).color(SOFT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(detail).color(SOFT));
                });
            });
        })
        .response
        .interact(Sense::click());
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// A small triangle: pointing right when closed, down when open. Drawn
/// rather than typed, because the built-in font has no arrow characters.
fn open_arrow(ui: &mut egui::Ui, open: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    let c = rect.center();
    let points = if open {
        vec![
            egui::pos2(c.x - 4.0, c.y - 2.0),
            egui::pos2(c.x + 4.0, c.y - 2.0),
            egui::pos2(c.x, c.y + 3.0),
        ]
    } else {
        vec![
            egui::pos2(c.x - 2.0, c.y - 4.0),
            egui::pos2(c.x + 3.0, c.y),
            egui::pos2(c.x - 2.0, c.y + 4.0),
        ]
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(points, DIM, Stroke::NONE));
}

fn pilot_details(ui: &mut egui::Ui, session: &StoredSession) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x24, 0x24, 0x24))
        .inner_margin(egui::Margin {
            left: 28,
            right: 10,
            top: 2,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new(("pilots", session.id))
                .num_columns(4)
                .spacing([8.0, 3.0])
                .min_col_width(56.0)
                .show(ui, |ui| {
                    for header in ["Pilot", "Mined", "m³", "ISK"] {
                        ui.label(small(header).color(DIM));
                    }
                    ui.end_row();
                    for pilot in &session.pilots {
                        ui.label(small(&pilot.character));
                        ui.label(
                            small(duration_text(pilot.ended_unix - pilot.started_unix)).color(SOFT),
                        );
                        ui.label(small(thousands(pilot.m3())));
                        ui.label(small(isk_text(pilot.isk())));
                        ui.end_row();
                    }
                });
            ui.label(small(ore_mix_text(session)).color(DIM));
        });
}

/// "Kylixium III-Grade 81% · Kylixium 19%" (the four largest ores).
fn ore_mix_text(session: &StoredSession) -> String {
    let mix = session.ore_mix();
    let total: f64 = mix.iter().map(|(_, m3)| m3).sum();
    if total <= 0.0 {
        return String::new();
    }
    mix.iter()
        .take(4)
        .map(|(ore, m3)| format!("{ore} {:.0}%", m3 / total * 100.0))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn period_list(ui: &mut egui::Ui, periods: &[PeriodTotal], weekly: bool) {
    if periods.is_empty() {
        ui.label(small("Nothing mined in this time.").color(DIM));
        return;
    }
    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::Grid::new(("periods", weekly))
            .num_columns(4)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for header in [
                    if weekly { "Week of" } else { "Day" },
                    "Sessions",
                    "m³",
                    "ISK",
                ] {
                    ui.label(small(header).color(DIM));
                }
                ui.end_row();
                for period in periods {
                    ui.label(date_text(period.start_unix));
                    ui.label(period.totals.sessions.to_string());
                    ui.label(thousands(period.totals.m3));
                    ui.label(RichText::new(isk_text(period.totals.isk)).color(BRIGHT));
                    ui.end_row();
                }
            });
    });
}

// ---------------------------------------------------------------------------
// Settings view

fn settings_view(module: &mut MiningSupervisor, ui: &mut egui::Ui) {
    let mut changed = false;
    let buffers = &mut module.edit_buffers;
    let settings = &mut module.settings;

    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new("Ore hold defaults (all characters)")
                .color(BRIGHT)
                .strong(),
        );
        egui::Grid::new("hold_defaults")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Ore hold (m³)");
                if let Some(Some(value)) =
                    number_field(ui, buffers, "hold", Some(settings.hold_m3), "11,500", 110.0)
                {
                    settings.hold_m3 = value;
                    changed = true;
                }
                ui.end_row();
                ui.label("Alert at (%)");
                if let Some(Some(value)) = number_field(
                    ui,
                    buffers,
                    "alert",
                    Some(settings.alert_percent),
                    "70",
                    110.0,
                ) {
                    settings.alert_percent = value.min(100.0);
                    changed = true;
                }
                ui.end_row();
            });
    });

    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Characters").color(BRIGHT).strong());
        ui.label(
            small("Found in your game logs. Leave a field empty to use the default above. Drag a row by its handle to change the order on the Live view.")
                .color(DIM),
        );
        let known: Vec<String> = module.trackers.keys().cloned().collect();
        let names = settings.ordered(known.iter());
        if names.is_empty() {
            ui.label(small("None yet.").color(DIM));
        }
        let hold_hint = thousands(settings.hold_m3);
        let alert_hint = format!("{:.0}", settings.alert_percent);

        // One row per character: drag handle, "show" box, name, hold, alert.
        // Fixed widths keep the columns lined up.
        const NAME_COLUMN: f32 = 170.0;
        ui.horizontal(|ui| {
            ui.add_space(14.0 + 24.0 + ui.spacing().item_spacing.x * 2.0);
            ui.allocate_ui_with_layout(
                Vec2::new(NAME_COLUMN, 14.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_width(NAME_COLUMN);
                    ui.label(small("Character").color(DIM));
                },
            );
            ui.add_sized([92.0, 14.0], egui::Label::new(small("Ore hold m³").color(DIM)));
            ui.add_sized([64.0, 14.0], egui::Label::new(small("Alert %").color(DIM)));
        });

        // Drag and drop: (dragged name, target name, after the target).
        let mut dropped: Option<(String, String, bool)> = None;
        let pointer_y = ui.ctx().pointer_interact_pos().map(|p| p.y);
        for name in &names {
            let mut own = settings.character(name);
            let drag_id = egui::Id::new(("mining_order", name));
            let being_dragged = ui.ctx().is_being_dragged(drag_id);
            let row = ui.horizontal(|ui| {
                if being_dragged {
                    // Faded while it's being moved.
                    ui.multiply_opacity(0.4);
                }
                let handle = drag_handle(ui, drag_id);
                // The payload is the dragged row's character name.
                handle.dnd_set_drag_payload(name.clone());
                if ui
                    .checkbox(&mut own.show, "")
                    .on_hover_text(format!("Show {name} on the Live view"))
                    .changed()
                {
                    changed = true;
                }
                name_cell(ui, name, NAME_COLUMN);
                if let Some(value) = number_field(
                    ui,
                    buffers,
                    &format!("hold:{name}"),
                    own.hold_m3,
                    &hold_hint,
                    92.0,
                ) {
                    own.hold_m3 = value;
                    changed = true;
                }
                if let Some(value) = number_field(
                    ui,
                    buffers,
                    &format!("alert:{name}"),
                    own.alert_percent,
                    &alert_hint,
                    64.0,
                ) {
                    own.alert_percent = value.map(|v| v.min(100.0));
                    changed = true;
                }
            });
            if own != settings.character(name) {
                settings.characters.insert(name.clone(), own);
            }

            let rect = row.response.rect;
            let after = pointer_y.is_some_and(|y| y > rect.center().y);
            if let Some(dragged) = row.response.dnd_hover_payload::<String>()
                && *dragged != *name
            {
                // Show where the row will land.
                let y = if after {
                    rect.bottom() + 2.0
                } else {
                    rect.top() - 2.0
                };
                ui.painter()
                    .hline(rect.x_range(), y, Stroke::new(2.0, BRIGHT));
            }
            if let Some(dragged) = row.response.dnd_release_payload::<String>() {
                dropped = Some(((*dragged).clone(), name.clone(), after));
            }
        }
        if let Some((dragged, target, after)) = dropped {
            settings.place_character(&known, &dragged, &target, after);
            changed = true;
        }
    });

    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Alerts").color(BRIGHT).strong());
        egui::Grid::new("alerts")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Idle alert after (min)");
                if let Some(Some(value)) =
                    number_field(ui, buffers, "idle", Some(settings.idle_minutes), "5", 110.0)
                {
                    settings.idle_minutes = value;
                    changed = true;
                }
                ui.end_row();
            });

        // One row per alert kind, with the colour it has in the log: is the
        // alert raised at all, and does it play a sound.
        egui::Frame::new()
            .fill(TILE_FILL)
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let rows = [
                    (
                        &mut settings.alert_crystal,
                        &mut settings.sound_crystal,
                        PURPLE,
                        "Crystal broke",
                    ),
                    (
                        &mut settings.alert_stopped,
                        &mut settings.sound_stopped,
                        RED,
                        "Miner stopped",
                    ),
                    (
                        &mut settings.alert_threshold,
                        &mut settings.sound_threshold,
                        YELLOW,
                        "Hold over alert level",
                    ),
                    (
                        &mut settings.alert_full,
                        &mut settings.sound_full,
                        RED,
                        "Hold full",
                    ),
                    (
                        &mut settings.alert_idle,
                        &mut settings.sound_idle,
                        DIM,
                        "Idle",
                    ),
                ];
                let sounds_on = settings.sound;
                egui::Grid::new("alert_kinds")
                    .num_columns(3)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(small("Alert type").color(DIM));
                        ui.label(small("Alert").color(DIM));
                        ui.label(small("Sound").color(DIM));
                        ui.end_row();
                        for (alert_on, sound_on, color, text) in rows {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(9.0, 9.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), 4.5, color);
                                ui.label(text);
                            });
                            if ui.checkbox(alert_on, "").changed() {
                                changed = true;
                            }
                            // The sound box is greyed out while the alert
                            // itself or all sounds are off.
                            let can_sound = *alert_on && sounds_on;
                            if ui
                                .add_enabled(can_sound, egui::Checkbox::without_text(sound_on))
                                .changed()
                            {
                                changed = true;
                            }
                            ui.end_row();
                        }
                    });
                ui.label(
                    small("Alert off: no line in the alert log and no sound. The cards still show the state.")
                        .color(DIM),
                );
            });

        if ui
            .checkbox(&mut settings.sound, "Play alert sounds")
            .changed()
        {
            changed = true;
        }
        egui::Grid::new("alert_quiet")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Quiet time after a sound (s)");
                if let Some(Some(value)) = number_field(
                    ui,
                    buffers,
                    "quiet",
                    Some(settings.quiet_seconds),
                    "10",
                    110.0,
                ) {
                    settings.quiet_seconds = value.min(600.0);
                    changed = true;
                }
                ui.end_row();
            });
        ui.label(
            small("After an alert sound, other alerts make no sound for this long. They still show in the alert log.")
                .color(DIM),
        );
        ui.label(
            small("\"Miner stopped\" is detected automatically from each character's cycle times.")
                .color(DIM),
        );
    });

    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Value").color(BRIGHT).strong());
        egui::Grid::new("value")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Reprocessing yield, ore/ice/moon (%)");
                if let Some(Some(value)) = number_field(
                    ui,
                    buffers,
                    "yore",
                    Some(settings.ore_yield_percent),
                    "90.63",
                    110.0,
                ) {
                    settings.ore_yield_percent = value.min(100.0);
                    changed = true;
                }
                ui.end_row();
                ui.label("Reprocessing yield, gas (%)");
                if let Some(Some(value)) = number_field(
                    ui,
                    buffers,
                    "ygas",
                    Some(settings.gas_yield_percent),
                    "95",
                    110.0,
                ) {
                    settings.gas_yield_percent = value.min(100.0);
                    changed = true;
                }
                ui.end_row();
                ui.label("Jita price");
                let before = settings.price_side;
                egui::ComboBox::from_id_salt("price_side")
                    .width(110.0)
                    .selected_text(match settings.price_side {
                        PriceSide::Buy => "Buy (highest)",
                        PriceSide::Sell => "Sell (lowest)",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut settings.price_side,
                            PriceSide::Buy,
                            "Buy (highest)",
                        );
                        ui.selectable_value(
                            &mut settings.price_side,
                            PriceSide::Sell,
                            "Sell (lowest)",
                        );
                    });
                if settings.price_side != before {
                    changed = true;
                }
                ui.end_row();
            });
        ui.label(
            small("Prices come from ESI market data and refresh at most once an hour.").color(DIM),
        );
    });

    section_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Live layout").color(BRIGHT).strong());
        ui.horizontal(|ui| {
            ui.label("Columns");
            for n in 1..=4u8 {
                if ui
                    .radio_value(&mut settings.columns, n, n.to_string())
                    .changed()
                {
                    changed = true;
                }
            }
        });
        ui.label(
            small("Each column of character cards is up to 480 px wide (at least 400 px). Make the Mining tab wider to fit them; if the tab is narrower, Athena shows as many columns as fit.")
                .color(DIM),
        );
    });

    if crate::overlay::is_supported() {
        section_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if overlay_settings(
                &mut settings.overlay,
                &mut module.overlay_move_mode,
                ui,
                buffers,
            ) {
                changed = true;
            }
        });
    }

    let static_text = match &module.static_data {
        Some(data) => format!("Ore data from SDE build {}.", data.sde_build),
        None => "No ore data loaded.".to_string(),
    };
    ui.label(small(static_text).color(Color32::from_rgb(0x8a, 0x8a, 0x8a)));

    if changed {
        module.settings_edited();
    }
}

/// The grip at the start of a character row in Settings: three short
/// lines, painted (the default font has no grip glyph). Drag it to move the
/// row.
fn drag_handle(ui: &mut egui::Ui, id: egui::Id) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 18.0), Sense::hover());
    let response = ui
        .interact(rect, id, Sense::drag())
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag to change the order");
    let color = if response.hovered() || response.dragged() {
        BRIGHT
    } else {
        DIM
    };
    for dy in [-4.0, 0.0, 4.0] {
        let y = rect.center().y + dy;
        ui.painter().hline(
            (rect.left() + 2.0)..=(rect.right() - 2.0),
            y,
            Stroke::new(1.5, color),
        );
    }
    response
}

/// A character name in a fixed-width cell, cut off when too long.
fn name_cell(ui: &mut egui::Ui, name: &str, width: f32) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 18.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.add(egui::Label::new(name).truncate());
        },
    );
}

/// A right-aligned number field. Returns `Some(new value)` once the user
/// leaves the field after changing it: `Some(None)` when it was emptied,
/// `None` when nothing changed or the text isn't a positive number (the
/// old value comes back then).
fn number_field(
    ui: &mut egui::Ui,
    buffers: &mut std::collections::BTreeMap<String, String>,
    key: &str,
    value: Option<f64>,
    hint: &str,
    width: f32,
) -> Option<Option<f64>> {
    let shown = value.map(plain_number).unwrap_or_default();
    let buffer = buffers
        .entry(key.to_string())
        .or_insert_with(|| shown.clone());
    let response = ui.add(
        egui::TextEdit::singleline(buffer)
            .hint_text(hint)
            .desired_width(width)
            .horizontal_align(Align::RIGHT),
    );
    let mut result = None;
    if response.lost_focus() {
        let text = buffer.trim().replace(',', "");
        if text.is_empty() {
            if value.is_some() {
                result = Some(None);
            }
        } else if let Ok(number) = text.parse::<f64>()
            && number > 0.0
            && Some(number) != value
        {
            result = Some(Some(number));
        }
    }
    if !response.has_focus() {
        // Show the value as stored (or as just changed).
        *buffer = match result {
            Some(new) => new.map(plain_number).unwrap_or_default(),
            None => shown,
        };
    }
    result
}

// ---------------------------------------------------------------------------
// Number, time and date formatting

fn now_unix() -> i64 {
    super::now().unix_seconds
}

/// "11,500" or "90.63": whole numbers with commas, others as they are.
fn plain_number(value: f64) -> String {
    if value.fract() == 0.0 {
        thousands(value)
    } else {
        format!("{value}")
    }
}

/// 112400.4 -> "112,400".
fn thousands(value: f64) -> String {
    let rounded = value.round() as i64;
    let digits = rounded.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if rounded < 0 { format!("-{out}") } else { out }
}

/// ISK the short way: "1.24 B", "498 M", "52 k".
fn isk_text(isk: f64) -> String {
    if isk >= 1e9 {
        format!("{:.2} B", isk / 1e9)
    } else if isk >= 1e6 {
        format!("{:.0} M", isk / 1e6)
    } else if isk >= 1e3 {
        format!("{:.0} k", isk / 1e3)
    } else {
        format!("{isk:.0}")
    }
}

/// m³ per second over `seconds` of mining. None for less than a minute,
/// where a cycle or two would give a wildly high number.
fn m3_per_second(m3: f64, seconds: i64) -> Option<f64> {
    if seconds < 60 {
        None
    } else {
        Some(m3 / seconds as f64)
    }
}

/// A rate per second with one decimal: "12.4", or "1,284.0" for whole belts.
fn per_second_text(per_second: f64) -> String {
    let tenths = (per_second * 10.0).round();
    format!(
        "{}.{}",
        thousands((tenths / 10.0).trunc()),
        (tenths % 10.0).abs() as i64
    )
}

/// m³ in belts: "24.28 M", "540,000".
fn short_m3(m3: f64) -> String {
    if m3 >= 1e6 {
        format!("{:.2} M", m3 / 1e6)
    } else {
        thousands(m3)
    }
}

/// m³ in the summary bar, short from 10,000 up: "9,874", "10.1k", "245k",
/// "5.29m", "14.2m". Three digits stay, so the text barely changes width.
fn short_volume(m3: f64) -> String {
    // The limits sit just below the round numbers, so 99,960 shows as
    // "100k" and not "100.0k".
    if m3 >= 99_950_000.0 {
        format!("{:.0}m", m3 / 1e6)
    } else if m3 >= 9_995_000.0 {
        format!("{:.1}m", m3 / 1e6)
    } else if m3 >= 999_500.0 {
        format!("{:.2}m", m3 / 1e6)
    } else if m3 >= 99_950.0 {
        format!("{:.0}k", m3 / 1e3)
    } else if m3 >= 10_000.0 {
        format!("{:.1}k", m3 / 1e3)
    } else {
        thousands(m3)
    }
}

/// How long ago something happened: "45 s ago", "12 min ago", "1h 05m ago".
fn ago_text(seconds: i64) -> String {
    format!("{} ago", duration_text(seconds.max(0)))
}

/// "149k", "1.18M".
fn compact(value: f64) -> String {
    if value >= 1e6 {
        format!("{:.2}M", value / 1e6)
    } else if value >= 1e3 {
        format!("{:.0}k", value / 1e3)
    } else {
        format!("{value:.0}")
    }
}

/// "2h 14m", "38 min", "45 s".
fn duration_text(seconds: i64) -> String {
    let minutes = seconds / 60;
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else if minutes >= 1 {
        format!("{minutes} min")
    } else {
        format!("{seconds} s")
    }
}

/// EVE time of day: "14:44".
fn clock_text(unix: i64) -> String {
    let seconds = unix.rem_euclid(86_400);
    format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

/// "Nov 8".
fn date_text(unix: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, month, day) = civil_from_days(unix.div_euclid(86_400));
    format!("{} {day}", MONTHS[(month - 1) as usize])
}

/// The "Mining overlay" box in Settings. Returns true when a setting changed.
fn overlay_settings(
    overlay: &mut OverlaySettings,
    move_mode: &mut bool,
    ui: &mut egui::Ui,
    buffers: &mut std::collections::BTreeMap<String, String>,
) -> bool {
    let mut changed = false;
    ui.label(RichText::new("Mining overlay").color(BRIGHT).strong());
    if ui
        .checkbox(&mut overlay.enabled, "Show hold bars overlay")
        .changed()
    {
        if !overlay.enabled {
            *move_mode = false;
        }
        changed = true;
    }
    ui.horizontal(|ui| {
        ui.label("Bar height (px)");
        if let Some(Some(value)) = number_field(
            ui,
            buffers,
            "bar_height",
            Some(overlay.bar_height_px),
            "6",
            60.0,
        ) {
            overlay.bar_height_px = value.round().clamp(2.0, 20.0);
            changed = true;
        }
    });
    changed |= ui
        .checkbox(
            &mut overlay.show_names,
            "Show character name under each bar",
        )
        .changed();
    changed |= ui
        .checkbox(
            &mut overlay.show_percent,
            "Show hold % under each bar (right side)",
        )
        .changed();
    egui::Grid::new("overlay_opacity")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Background opacity");
            changed |= ui
                .add(
                    egui::Slider::new(&mut overlay.background_opacity, 0.0..=100.0)
                        .suffix("%")
                        .fixed_decimals(0),
                )
                .changed();
            ui.end_row();
            ui.label("Bar opacity");
            changed |= ui
                .add(
                    egui::Slider::new(&mut overlay.bar_opacity, 10.0..=100.0)
                        .suffix("%")
                        .fixed_decimals(0),
                )
                .changed();
            ui.end_row();
        });
    let enabled = overlay.enabled;
    ui.horizontal(|ui| {
        let moving = *move_mode;
        let label = if moving {
            "Done"
        } else {
            "Move / resize overlay"
        };
        if ui
            .add_enabled(enabled, egui::Button::new(label).selected(moving))
            .clicked()
        {
            *move_mode = !moving;
        }
        let hint = if *move_mode {
            "Drag the overlay to move it, its right edge to change the width."
        } else {
            "Clicks go through the overlay."
        };
        ui.label(small(hint).color(DIM));
    });
    ui.label(
        small(
            "Bars follow the character order above; hidden characters get no bar. \
             Position and width are saved and come back after a restart.",
        )
        .color(DIM),
    );
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_count_fits_the_tab() {
        let fit = |setting, available| {
            let c = live_columns(setting, available);
            (c.count, c.width)
        };
        // Room to spare: full 480 px columns.
        assert_eq!(fit(2, 2000.0), (2, 480.0));
        assert_eq!(live_columns(2, 2000.0).total_width(), 966.0);
        // Four on a 1080p screen: the columns shrink a little.
        assert_eq!(fit(4, 1900.0), (4, 470.5));
        // Four wanted, but the tab only has room for two.
        assert_eq!(fit(4, 1000.0), (2, 480.0));
        assert_eq!(fit(4, 900.0), (2, 447.0));
        // Always at least one, even in a narrow tab.
        assert_eq!(fit(3, 300.0), (1, 300.0));
        assert_eq!(fit(1, 2000.0), (1, 480.0));
    }

    #[test]
    fn number_formats() {
        assert_eq!(thousands(112_400.4), "112,400");
        assert_eq!(thousands(999.0), "999");
        assert_eq!(thousands(1_000.0), "1,000");
        assert_eq!(isk_text(1_240_000_000.0), "1.24 B");
        assert_eq!(isk_text(498_000_000.0), "498 M");
        assert_eq!(short_m3(24_280_000.0), "24.28 M");
        assert_eq!(compact(149_000.0), "149k");
        // Kevin's examples, plus the edges.
        assert_eq!(short_volume(10_124.0), "10.1k");
        assert_eq!(short_volume(10_124_124.0), "10.1m");
        assert_eq!(short_volume(9_874.0), "9,874");
        assert_eq!(short_volume(99_960.0), "100k");
        assert_eq!(short_volume(245_400.0), "245k");
        assert_eq!(short_volume(999_700.0), "1.00m");
        assert_eq!(short_volume(5_290_000.0), "5.29m");
        assert_eq!(short_volume(9_996_000.0), "10.0m");
        assert_eq!(short_volume(124_000_000.0), "124m");
        assert_eq!(plain_number(11_500.0), "11,500");
        assert_eq!(plain_number(90.63), "90.63");
    }

    #[test]
    fn per_second_rates() {
        // 771,064 m³/h on a belt is about 214.2 m³/s.
        assert_eq!(per_second_text(771_064.0 / 3600.0), "214.2");
        assert_eq!(per_second_text(12.44), "12.4");
        assert_eq!(per_second_text(12.96), "13.0");
        assert_eq!(per_second_text(0.0), "0.0");
        assert_eq!(per_second_text(4_621.25), "4,621.3");
        // 45,000 m³ over an hour of mining is 12.5 m³/s.
        assert_eq!(m3_per_second(45_000.0, 3600), Some(12.5));
        // Under a minute there isn't enough to go on.
        assert_eq!(m3_per_second(3_000.0, 59), None);
    }

    #[test]
    fn time_formats() {
        assert_eq!(duration_text(2 * 3600 + 14 * 60), "2h 14m");
        assert_eq!(duration_text(38 * 60), "38 min");
        assert_eq!(ago_text(45), "45 s ago");
        assert_eq!(ago_text(12 * 60 + 5), "12 min ago");
        assert_eq!(ago_text(3600 + 5 * 60), "1h 05m ago");
        // A clock a little off never shows a negative time.
        assert_eq!(ago_text(-3), "0 s ago");
        // 2026-11-08 14:44 UTC.
        let t = 1_794_096_000 + 14 * 3600 + 44 * 60;
        assert_eq!(clock_text(t), "14:44");
        assert_eq!(date_text(t), "Nov 8");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(date_text(1_794_096_000 - 86_400 * 8), "Oct 31");
    }
}
