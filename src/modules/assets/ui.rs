//! Draws the Assets panel from the mockup Kevin approved
//! (<https://claude.ai/artifact/KqyP133XTJegF5Lnx4MsH9>): a toolbar (sort,
//! route start, characters, search), the filter chips, how old the data
//! is, and the location list with a totals line.
//!
//! The list draws only the rows on screen (every row is equally tall), so
//! thousands of items cost nothing while they are scrolled away. Clicks
//! are collected while drawing and applied afterwards.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense,
    Stroke, Vec2, text::LayoutJob, text::TextFormat,
};

use super::Assets;
use super::config::{PriceSource, RouteFrom, SortBy, Who};
use super::data::{LocationKey, Row};
use crate::modules::finance::data::short_isk;
use crate::modules::intel::map::security_color;

pub(super) const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
pub(super) const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
pub(super) const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
pub(super) const FAINT: Color32 = Color32::from_rgb(0x6a, 0x6a, 0x6a);
pub(super) const YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x47);
const GREEN: Color32 = Color32::from_rgb(0x5c, 0xbf, 0x6a);
pub(super) const ACCENT: Color32 = Color32::from_rgb(0x2c, 0x7d, 0xa3);
pub(super) const ACCENT_FILL: Color32 = Color32::from_rgb(0x0f, 0x3a, 0x4d);
const LIST_FILL: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
const HEAD_FILL: Color32 = Color32::from_rgb(0x1b, 0x1b, 0x1b);
const OPEN_FILL: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
const HOVER_FILL: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
const BORDER: Color32 = Color32::from_rgb(0x35, 0x35, 0x35);
const TREE_LINE: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
const OWNER_FILL: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x38);
const OWNER_TEXT: Color32 = Color32::from_rgb(0xa9, 0xc4, 0xe8);

/// The list is at most this wide: two of Athena's 480 px columns.
const LIST_MAX_WIDTH: f32 = 960.0;
/// Every row is this tall; a ship or container takes two.
const ROW_HEIGHT: f32 = 20.0;
/// Indent per level inside a location.
const INDENT: f32 = 16.0;
const SIDE_PAD: f32 = 10.0;
/// Columns of a location row, from the left...
const ARROW_W: f32 = 14.0;
const SEC_W: f32 = 34.0;
const SYSTEM_W: f32 = 92.0;
/// ... and from the right.
const ITEMS_W: f32 = 64.0;
const ISK_W: f32 = 72.0;
const M3_W: f32 = 72.0;
const JUMPS_W: f32 = 56.0;
const GAP: f32 = 8.0;
/// The owner's name tag is at most this wide.
const OWNER_MAX_W: f32 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    List,
    Settings,
}

/// What the panel remembers between frames (not saved).
#[derive(Debug, Default)]
pub struct UiState {
    pub tab: Tab,
    /// What is typed in the "Route from" system box.
    pub system_text: String,
}

/// A click on a row, applied after drawing.
enum Click {
    Location(LocationKey),
    Node(i64),
}

pub(super) fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

/// In a wrapping row, starts a new line unless a button with this small
/// text (plus `extra` points) still fits. Needed for buttons drawn with
/// `add_outlined`, which a wrapping row can't move to the next line by
/// itself.
pub(super) fn make_room(ui: &mut egui::Ui, text: &str, extra: f32) {
    let text_width = ui
        .fonts_mut(|f| f.layout_no_wrap(text.to_string(), FontId::proportional(12.0), TEXT))
        .size()
        .x;
    let width = text_width + 2.0 * ui.spacing().button_padding.x + 2.0 + extra;
    if ui.available_size_before_wrap().x < width {
        ui.end_row();
    }
}

/// "14:05" in local time.
fn hour_minute(unix: i64) -> String {
    let mut text = crate::localtime::clock_text(unix);
    text.truncate(5);
    text
}

/// "just now", "23 min ago", "2 h ago".
fn ago_text(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes} min ago")
    } else {
        format!("{} h ago", minutes / 60)
    }
}

/// "1,234".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Like the ISK numbers, without the space: "1.71m", "500".
fn short_m3(value: f64) -> String {
    short_isk(value).replace(' ', "")
}

fn isk_text(value: f64) -> String {
    short_isk(value).replace(' ', "")
}

/// The panel.
pub fn show(module: &mut Assets, ui: &mut egui::Ui, now: i64) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    tabs(module, ui, now);
    for problem in &module.problems {
        ui.colored_label(YELLOW, small(problem));
    }
    if module.ui.tab == Tab::Settings {
        settings_view(module, ui);
        return;
    }
    if !crate::auth::is_configured() {
        ui.colored_label(YELLOW, "EVE login isn't set up in this build.");
        return;
    }
    if module.characters.is_empty() {
        ui.label("Log in your characters on the Characters tab, and their assets show up here.");
        return;
    }
    if module.static_data.is_none() {
        return;
    }
    toolbar(module, ui);
    if module.settings.show_filters {
        chips(module, ui);
    }
    status_line(module, ui, now);
    list(module, ui);
}

fn tabs(module: &mut Assets, ui: &mut egui::Ui, now: i64) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.ui.tab, Tab::List, "Assets");
        ui.selectable_value(&mut module.ui.tab, Tab::Settings, "Settings");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(module.can_refresh(now), egui::Button::new(small("Refresh")))
                .on_hover_text("Ask ESI for every character's assets now.")
                .on_disabled_hover_text("Just refreshed. Try again in a minute.")
                .clicked()
            {
                module.refresh_now();
            }
        });
    });
}

fn toolbar(module: &mut Assets, ui: &mut egui::Ui) {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(small("Sort by").color(FAINT));
        egui::ComboBox::from_id_salt("assets_sort")
            .selected_text(module.settings.sort.label())
            .show_ui(ui, |ui| {
                for sort in SortBy::ALL {
                    changed |= ui
                        .selectable_value(&mut module.settings.sort, sort, sort.label())
                        .changed();
                }
            });
        let arrow = if module.settings.reversed {
            "⏷"
        } else {
            "⏶"
        };
        if ui
            .button(arrow)
            .on_hover_text("Reverse the order")
            .clicked()
        {
            module.settings.reversed = !module.settings.reversed;
            changed = true;
        }

        ui.add_space(6.0);
        ui.label(small("Route from").color(FAINT));
        changed |= route_picker(module, ui);

        ui.add_space(6.0);
        ui.label(small("Characters").color(FAINT));
        changed |= who_picker(module, ui);

        ui.add_space(6.0);
        let search = ui.add(
            egui::TextEdit::singleline(&mut module.search)
                .hint_text("Search items, ships, systems…")
                .desired_width(210.0),
        );
        if search.changed() {
            module.view_dirty = true;
        }
        let label = if module.settings.show_filters {
            "Filters ⏶"
        } else {
            "Filters ⏷"
        };
        let on = module.settings.show_filters;
        let button = egui::Button::new(small(label)).fill(if on {
            ACCENT_FILL
        } else {
            Color32::from_gray(0x2c)
        });
        if crate::widgets::add_outlined(ui, true, button, Stroke::new(1.0, ACCENT)).clicked() {
            module.settings.show_filters = !on;
            module.save_settings();
        }
    });
    if changed {
        module.settings_changed();
    }
}

/// The "Route from" picker. True when the setting changed.
fn route_picker(module: &mut Assets, ui: &mut egui::Ui) -> bool {
    let universe = module.static_data.as_ref().map(|s| &s.universe);
    let system_name = |id: i64| {
        universe
            .and_then(|u| u.system(id))
            .map_or_else(|| "?".to_string(), |s| s.name.clone())
    };
    let current = module.settings.route_from.or_else(|| {
        module
            .characters
            .first()
            .map(|c| RouteFrom::Character(c.id))
    });
    let selected = match current {
        Some(RouteFrom::Character(id)) => {
            let place = module
                .route_system
                .map_or_else(|| "where?".to_string(), system_name);
            format!("{} ({place})", module.character_name(id))
        }
        Some(RouteFrom::System(_)) => "A system".to_string(),
        None => "–".to_string(),
    };
    let mut picked = current;
    egui::ComboBox::from_id_salt("assets_route_from")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for c in &module.characters {
                ui.selectable_value(&mut picked, Some(RouteFrom::Character(c.id)), &c.name);
            }
            let system = match current {
                Some(RouteFrom::System(id)) => id,
                // Start the system box where the route is now.
                _ => module.route_system.unwrap_or(30000142),
            };
            ui.selectable_value(&mut picked, Some(RouteFrom::System(system)), "A system…");
        });
    let mut changed = false;
    if picked != current {
        module.settings.route_from = picked;
        if let Some(RouteFrom::System(id)) = picked {
            module.ui.system_text = system_name(id);
        }
        changed = true;
    }
    if let Some(RouteFrom::System(id)) = module.settings.route_from {
        if module.ui.system_text.is_empty() {
            module.ui.system_text = system_name(id);
        }
        let edit = ui.add(
            egui::TextEdit::singleline(&mut module.ui.system_text)
                .hint_text("System name")
                .desired_width(90.0),
        );
        let found = universe.and_then(|u| u.system_by_name(module.ui.system_text.trim()));
        match found {
            Some(system) if system.id != id => {
                module.settings.route_from = Some(RouteFrom::System(system.id));
                changed = true;
            }
            Some(_) => {}
            None if edit.has_focus() || !module.ui.system_text.trim().is_empty() => {
                ui.label(small("not found").color(YELLOW));
            }
            None => {}
        }
    }
    changed
}

/// The "Characters" picker. True when it changed.
fn who_picker(module: &mut Assets, ui: &mut egui::Ui) -> bool {
    let mut tags: Vec<String> = module
        .characters
        .iter()
        .flat_map(|c| c.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();
    let selected = match &module.settings.who {
        Who::All => format!("All ({})", module.characters.len()),
        Who::Tag(tag) => format!("Tag: {tag}"),
        Who::Character(id) => module.character_name(*id),
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt("assets_who")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            let who = &mut module.settings.who;
            changed |= ui
                .selectable_value(who, Who::All, "All characters")
                .changed();
            for tag in tags {
                let label = format!("Tag: {tag}");
                changed |= ui.selectable_value(who, Who::Tag(tag), label).changed();
            }
            for c in &module.characters {
                changed |= ui
                    .selectable_value(who, Who::Character(c.id), &c.name)
                    .changed();
            }
        });
    changed
}

/// The saved filters as chips: the name turns one on or off, the gear
/// edits it.
fn chips(module: &mut Assets, ui: &mut egui::Ui) {
    let mut toggle = None;
    let mut edit = None;
    let mut new = false;
    let mut clear = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (i, filter) in module.settings.filters.iter().enumerate() {
            let on = module.settings.active.contains(&i);
            // The name plus the gear button.
            make_room(ui, &filter.name, 30.0);
            ui.scope(|ui| {
                // The two halves of a chip touch.
                ui.spacing_mut().item_spacing.x = 0.0;
                let (fill, stroke, color) = if on {
                    (ACCENT_FILL, Stroke::new(1.0, ACCENT), BRIGHT)
                } else {
                    (
                        Color32::from_gray(0x2c),
                        Stroke::new(1.0, Color32::from_gray(0x4a)),
                        TEXT,
                    )
                };
                let name = egui::Button::new(small(&filter.name).color(color))
                    .fill(fill)
                    .corner_radius(CornerRadius {
                        nw: 4,
                        sw: 4,
                        ne: 0,
                        se: 0,
                    });
                if crate::widgets::add_outlined(ui, true, name, stroke)
                    .on_hover_text("Show only what this filter matches")
                    .clicked()
                {
                    toggle = Some(i);
                }
                let gear = egui::Button::new(small("⚙").color(DIM))
                    .fill(Color32::from_gray(0x2c))
                    .corner_radius(CornerRadius {
                        nw: 0,
                        sw: 0,
                        ne: 4,
                        se: 4,
                    });
                if crate::widgets::add_outlined(ui, true, gear, stroke)
                    .on_hover_text("Edit this filter")
                    .clicked()
                {
                    edit = Some(i);
                }
            });
        }
        let button =
            egui::Button::new(small("+ New filter").color(DIM)).fill(Color32::from_gray(0x24));
        if crate::widgets::add_outlined(
            ui,
            true,
            button,
            Stroke::new(1.0, Color32::from_gray(0x4a)),
        )
        .clicked()
        {
            new = true;
        }
        if !module.settings.active.is_empty() && ui.button(small("Clear")).clicked() {
            clear = true;
        }
    });
    if let Some(i) = toggle {
        module.settings.toggle_filter(i);
        module.settings_changed();
    }
    if let Some(i) = edit {
        module.editor = Some(super::filter_ui::Editor::edit(
            i,
            &module.settings.filters[i],
        ));
    }
    if new {
        module.editor = Some(super::filter_ui::Editor::new_filter());
    }
    if clear {
        module.settings.active.clear();
        module.settings_changed();
    }
}

/// How old the data is, what is being fetched, and problems.
fn status_line(module: &Assets, ui: &mut egui::Ui, now: i64) {
    let owners = module.owners();
    let shown: Vec<_> = module
        .assets
        .iter()
        .filter(|(id, _)| owners.contains(id))
        .map(|(_, a)| a)
        .collect();
    ui.horizontal_wrapped(|ui| {
        let oldest = shown.iter().map(|a| a.data_from).min();
        let next = shown.iter().map(|a| a.next_at).min();
        match oldest {
            Some(from) => {
                let old = now - from > 2 * 3600;
                // A painted dot: the font has no "●".
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 4.0, if old { YELLOW } else { GREEN });
                let mut text = format!(
                    "ESI data from {} ({})",
                    hour_minute(from),
                    ago_text(now - from)
                );
                if let Some(next) = next.filter(|n| *n > now) {
                    text.push_str(&format!(", next update {}", hour_minute(next)));
                }
                ui.label(small(text).color(DIM));
            }
            None if module.loaded => {
                ui.label(small("No assets fetched yet.").color(DIM));
            }
            None => {
                ui.label(small("Reading saved assets…").color(DIM));
            }
        }
        if let Some(prices) = &module.prices {
            let source = match prices.source {
                PriceSource::Average => "EVE average",
                PriceSource::JitaSell => "Jita sell",
                PriceSource::JitaBuy => "Jita buy",
            };
            ui.label(
                small(format!(
                    "· prices from {} ({source})",
                    hour_minute(prices.fetched_at)
                ))
                .color(DIM),
            );
        }
        if !module.fetching.is_empty() {
            ui.label(
                small(format!(
                    "· fetching {} character(s)…",
                    module.fetching.len()
                ))
                .color(DIM),
            );
        }
        if module.prices_busy {
            ui.label(small("· fetching prices…").color(DIM));
        }
    });
    for c in &module.characters {
        if owners.contains(&c.id)
            && let Some(problem) = module.character_problems.get(&c.id)
        {
            ui.colored_label(YELLOW, small(format!("{}: {problem}", c.name)));
        }
    }
}

/// The location list with its totals line and column titles.
fn list(module: &mut Assets, ui: &mut egui::Ui) {
    let width = ui.available_width().min(LIST_MAX_WIDTH);
    let mut clicks: Vec<Click> = Vec::new();
    egui::Frame::new()
        .fill(LIST_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            totals(module, ui, width);
            header(ui, width);
            if module.rows.is_empty() {
                ui.add_space(8.0);
                let text = if module.view.narrowed {
                    "Nothing matches. Clear the search or turn a filter off."
                } else if module.assets.is_empty() {
                    "No assets yet. They are fetched while this tab is open."
                } else {
                    "These characters own nothing."
                };
                ui.horizontal(|ui| {
                    ui.add_space(SIDE_PAD);
                    ui.label(small(text).italics().color(FAINT));
                });
                ui.add_space(8.0);
                return;
            }
            // Every owner tag is as wide as the longest name, so the ship
            // names line up.
            let tag_width = module
                .characters
                .iter()
                .map(|c| tag_width(ui, &c.name))
                .fold(0.0, f32::max);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW_HEIGHT, module.rows.len(), |ui, range| {
                    for index in range {
                        if let Some(click) = row(module, ui, width, tag_width, index) {
                            clicks.push(click);
                        }
                    }
                });
        });
    for click in clicks {
        match click {
            Click::Location(key) => module.toggle_location(key),
            Click::Node(item_id) => module.toggle_node(item_id),
        }
    }
}

fn totals(module: &Assets, ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius {
            nw: 4,
            ne: 4,
            sw: 0,
            se: 0,
        },
        HEAD_FILL,
    );
    let v = &module.view;
    let mut job = LayoutJob::default();
    let font = FontId::proportional(12.0);
    let mut part = |text: &str, color: Color32| {
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };
    part(&thousands(v.locations.len() as u64), TEXT);
    part(" locations    ", DIM);
    part(&thousands(u64::from(v.count)), TEXT);
    part(" items    ", DIM);
    part(&isk_text(v.value), TEXT);
    part(" ISK    ", DIM);
    part(&short_m3(v.volume), TEXT);
    part(" m³", DIM);
    if v.narrowed {
        part("    filtered", YELLOW);
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    painter.galley(
        Pos2::new(
            rect.left() + SIDE_PAD,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        TEXT,
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, BORDER),
    );
}

/// Where each column of a location row is.
struct Columns {
    sec: Rect,
    system: Rect,
    station: Rect,
    items: Rect,
    isk: Rect,
    m3: Rect,
    jumps: Rect,
}

fn columns(rect: Rect) -> Columns {
    let y = rect.y_range();
    let span = |left: f32, w: f32| Rect::from_x_y_ranges(left..=left + w, y);
    let left = rect.left() + SIDE_PAD + ARROW_W + GAP;
    let right = rect.right() - SIDE_PAD;
    let jumps = span(right - JUMPS_W, JUMPS_W);
    let m3 = span(jumps.left() - GAP - M3_W, M3_W);
    let isk = span(m3.left() - GAP - ISK_W, ISK_W);
    let items = span(isk.left() - GAP - ITEMS_W, ITEMS_W);
    let sec = span(left, SEC_W);
    let system = span(sec.right() + GAP, SYSTEM_W);
    let station_left = system.right() + GAP;
    let station = span(station_left, (items.left() - GAP - station_left).max(20.0));
    Columns {
        sec,
        system,
        station,
        items,
        isk,
        m3,
        jumps,
    }
}

fn header(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, HEAD_FILL);
    let c = columns(rect);
    let font = FontId::proportional(11.0);
    let left = |r: Rect, text: &str| {
        painter.text(
            Pos2::new(r.left(), r.center().y),
            Align2::LEFT_CENTER,
            text,
            font.clone(),
            FAINT,
        );
    };
    let right = |r: Rect, text: &str| {
        painter.text(
            Pos2::new(r.right(), r.center().y),
            Align2::RIGHT_CENTER,
            text,
            font.clone(),
            FAINT,
        );
    };
    left(c.sec, "SEC");
    left(c.system, "SYSTEM");
    left(c.station, "STATION / STRUCTURE");
    right(c.items, "ITEMS");
    right(c.isk, "ISK");
    right(c.m3, "M³");
    right(c.jumps, "JUMPS");
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, BORDER),
    );
}

/// Text in a cell, cut off at its edge.
fn cell(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: &FontId,
    color: Color32,
    right: bool,
) {
    let clipped = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let (pos, align) = if right {
        (
            Pos2::new(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
        )
    } else {
        (Pos2::new(rect.left(), rect.center().y), Align2::LEFT_CENTER)
    };
    clipped.text(pos, align, text, font.clone(), color);
}

/// How wide the owner tag for this name is.
fn tag_width(ui: &egui::Ui, name: &str) -> f32 {
    let font = FontId::proportional(11.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(name.to_string(), font, OWNER_TEXT));
    (galley.size().x + 10.0).min(OWNER_MAX_W)
}

/// A small tag with the owner's name, where RIFT shows a portrait. Returns
/// its right edge.
fn owner_tag(
    ui: &egui::Ui,
    painter: &egui::Painter,
    left: f32,
    center_y: f32,
    width: f32,
    name: &str,
) -> f32 {
    let font = FontId::proportional(11.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(name.to_string(), font, OWNER_TEXT));
    let rect = Rect::from_min_size(Pos2::new(left, center_y - 8.0), Vec2::new(width, 16.0));
    painter.rect_filled(rect, 3.0, OWNER_FILL);
    let clipped = painter.with_clip_rect(
        rect.shrink2(Vec2::new(4.0, 0.0))
            .intersect(painter.clip_rect()),
    );
    clipped.galley(
        Pos2::new(rect.left() + 5.0, center_y - galley.size().y / 2.0),
        galley,
        OWNER_TEXT,
    );
    rect.right()
}

/// Draws one row. Returns a click on it.
fn row(module: &Assets, ui: &mut egui::Ui, width: f32, tag_w: f32, index: usize) -> Option<Click> {
    let row = module.rows.get(index)?;
    let clickable = match row {
        Row::Location(_) => true,
        Row::BigFirst { node, .. } | Row::BigSecond { node, .. } => {
            !module.tree.nodes[*node as usize].children.is_empty()
        }
        _ => false,
    };
    let sense = if clickable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), sense);
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let painter = ui.painter().clone();
    let font = FontId::proportional(13.0);
    let small_font = FontId::proportional(12.0);
    let hovered = clickable && response.hovered();
    match row {
        Row::Location(i) => {
            let lv = &module.view.locations[*i];
            let location = &module.tree.locations[lv.location];
            let open = row_is_open(module, index);
            if hovered {
                painter.rect_filled(rect, 0.0, HOVER_FILL);
            } else if open {
                painter.rect_filled(rect, 0.0, OPEN_FILL);
            }
            let c = columns(rect);
            arrow(&painter, rect.left() + SIDE_PAD, rect.center().y, open);
            let universe = module.static_data.as_ref().map(|s| &s.universe);
            let system = location.system_id.and_then(|id| universe?.system(id));
            match system {
                Some(s) => {
                    let sec = s.display_security();
                    cell(
                        &painter,
                        c.sec,
                        &format!("{sec:.1}"),
                        &FontId::monospace(12.0),
                        security_color(sec),
                        false,
                    );
                    cell(&painter, c.system, &s.name, &font, BRIGHT, false);
                }
                None => {
                    cell(&painter, c.sec, "?", &FontId::monospace(12.0), FAINT, false);
                    cell(&painter, c.system, "unknown", &font, DIM, false);
                }
            }
            cell(&painter, c.station, &location.name, &font, TEXT, false);
            cell(
                &painter,
                c.items,
                &thousands(u64::from(lv.count)),
                &font,
                TEXT,
                true,
            );
            cell(&painter, c.isk, &isk_text(lv.value), &font, TEXT, true);
            cell(&painter, c.m3, &short_m3(lv.volume), &font, DIM, true);
            let jumps = lv.jumps.map_or_else(|| "–".to_string(), |j| j.to_string());
            cell(&painter, c.jumps, &jumps, &font, DIM, true);
            if response.clicked() {
                return Some(Click::Location(location.key));
            }
        }
        Row::BigFirst { node, depth } | Row::BigSecond { node, depth } => {
            let first = matches!(row, Row::BigFirst { .. });
            let n = &module.tree.nodes[*node as usize];
            if hovered {
                painter.rect_filled(rect, 0.0, HOVER_FILL);
            }
            let left = indent_left(&painter, rect, *depth);
            let tag_left = left + ARROW_W;
            // The owner tag is as wide on both lines, so line 2 starts
            // under the name.
            let owner = module.character_name(n.owner);
            if first {
                if !n.children.is_empty() {
                    let open = row_is_open(module, index);
                    arrow(&painter, left, rect.center().y, open);
                }
                let text_left =
                    owner_tag(ui, &painter, tag_left, rect.center().y, tag_w, &owner) + 8.0;
                let mut job = LayoutJob::default();
                let fmt = |color: Color32| TextFormat {
                    font_id: font.clone(),
                    color,
                    ..Default::default()
                };
                let type_name = module.type_name(n.type_id);
                let title = n.name.clone().unwrap_or_else(|| type_name.clone());
                job.append(&title, 0.0, fmt(BRIGHT));
                if n.name.is_some() {
                    job.append(&format!(" – {type_name}"), 0.0, fmt(DIM));
                }
                if !n.is_ship {
                    job.append("  container", 0.0, fmt(FAINT));
                }
                job.append(
                    &format!(
                        "  ·  {} m³  ·  {} ISK",
                        short_m3(n.volume),
                        isk_text(n.value)
                    ),
                    0.0,
                    fmt(DIM),
                );
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                let clip =
                    Rect::from_x_y_ranges(text_left..=rect.right() - SIDE_PAD, rect.y_range());
                painter
                    .with_clip_rect(clip.intersect(painter.clip_rect()))
                    .galley(
                        Pos2::new(text_left, rect.center().y - galley.size().y / 2.0),
                        galley,
                        TEXT,
                    );
            } else {
                let text_left = tag_left + tag_w + 8.0;
                let inside = n.count.saturating_sub(1);
                let contents_volume: f64 = n
                    .children
                    .iter()
                    .map(|c| module.tree.nodes[*c as usize].volume)
                    .sum();
                let text = if inside == 0 {
                    "empty".to_string()
                } else {
                    format!(
                        "{} {}  ·  {} m³  ·  {} ISK",
                        thousands(u64::from(inside)),
                        if inside == 1 { "item" } else { "items" },
                        short_m3(contents_volume),
                        isk_text(n.value - n.own_value)
                    )
                };
                cell(
                    &painter,
                    Rect::from_x_y_ranges(text_left..=rect.right() - SIDE_PAD, rect.y_range()),
                    &text,
                    &small_font,
                    DIM,
                    false,
                );
            }
            if response.clicked() {
                return Some(Click::Node(n.item_id));
            }
        }
        Row::Section { title, depth } => {
            let left = indent_left(&painter, rect, *depth) + ARROW_W;
            cell(
                &painter,
                Rect::from_x_y_ranges(left..=rect.right(), rect.y_range()),
                &title.to_uppercase(),
                &FontId::proportional(10.5),
                FAINT,
                false,
            );
        }
        Row::Item { node, depth, owner } => {
            let n = &module.tree.nodes[*node as usize];
            let left = indent_left(&painter, rect, *depth) + ARROW_W;
            let c = columns(rect);
            let mut text_left = left;
            if *owner {
                text_left = owner_tag(
                    ui,
                    &painter,
                    left,
                    rect.center().y,
                    tag_w,
                    &module.character_name(n.owner),
                ) + 8.0;
            }
            let mut job = LayoutJob::default();
            let fmt = |font: &FontId, color: Color32| TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            };
            let name = match &n.name {
                Some(given) => format!("{given} ({})", module.type_name(n.type_id)),
                None => module.type_name(n.type_id),
            };
            job.append(&name, 0.0, fmt(&small_font, TEXT));
            job.append(
                &format!("  × {}", thousands(n.quantity.max(0) as u64)),
                0.0,
                fmt(&small_font, DIM),
            );
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let clip = Rect::from_x_y_ranges(text_left..=c.isk.left() - GAP, rect.y_range());
            painter
                .with_clip_rect(clip.intersect(painter.clip_rect()))
                .galley(
                    Pos2::new(text_left, rect.center().y - galley.size().y / 2.0),
                    galley,
                    TEXT,
                );
            cell(
                &painter,
                c.isk,
                &format!("{} ISK", isk_text(n.value)),
                &small_font,
                DIM,
                true,
            );
            cell(
                &painter,
                c.m3,
                &format!("{} m³", short_m3(n.volume)),
                &small_font,
                DIM,
                true,
            );
        }
    }
    if clickable {
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    None
}

/// The open/closed triangle in front of a row, painted (not every font
/// has the arrow characters).
fn arrow(painter: &egui::Painter, left: f32, center_y: f32, open: bool) {
    let x = left + 4.0;
    let points = if open {
        vec![
            Pos2::new(x, center_y - 2.5),
            Pos2::new(x + 7.0, center_y - 2.5),
            Pos2::new(x + 3.5, center_y + 3.0),
        ]
    } else {
        vec![
            Pos2::new(x + 1.0, center_y - 4.0),
            Pos2::new(x + 6.5, center_y),
            Pos2::new(x + 1.0, center_y + 4.0),
        ]
    };
    painter.add(egui::Shape::convex_polygon(points, FAINT, Stroke::NONE));
}

/// Draws the tree line for a row inside a location and returns where its
/// content starts.
fn indent_left(painter: &egui::Painter, rect: Rect, depth: u8) -> f32 {
    let line_x = rect.left() + SIDE_PAD + 6.0;
    painter.vline(line_x, rect.y_range(), Stroke::new(1.0, TREE_LINE));
    rect.left() + SIDE_PAD + f32::from(depth) * INDENT - 4.0
}

/// Whether the row at `index` is open: the next row is one level deeper.
fn row_is_open(module: &Assets, index: usize) -> bool {
    let depth_of = |row: &Row| match row {
        Row::Location(_) => 0,
        Row::BigFirst { depth, .. }
        | Row::BigSecond { depth, .. }
        | Row::Section { depth, .. }
        | Row::Item { depth, .. } => *depth,
    };
    let Some(this) = module.rows.get(index) else {
        return false;
    };
    // A ship's first line is followed by its second; look past it.
    let next = if matches!(this, Row::BigFirst { .. }) {
        index + 2
    } else {
        index + 1
    };
    module
        .rows
        .get(next)
        .is_some_and(|row| depth_of(row) > depth_of(this))
}

fn settings_view(module: &mut Assets, ui: &mut egui::Ui) {
    ui.set_max_width(480.0);
    ui.label(RichText::new("Prices").strong());
    let mut source = module.settings.price_source;
    for option in PriceSource::ALL {
        ui.radio_value(&mut source, option, small(option.label()));
    }
    module.set_price_source(source);
    ui.label(
        small(
            "The average price is CCP's average over recent trades, the same as the \
             in-game \"Estimated price\". The Jita prices need one ESI request per item \
             type, so the first fetch can take a few minutes. Prices are fetched again \
             every hour. Blueprint copies count as 0 ISK.",
        )
        .color(DIM),
    );
    ui.add_space(8.0);
    ui.label(RichText::new("Fetching").strong());
    ui.label(
        small(
            "Assets are fetched only while the Assets tab is open, and again when ESI's \
             copy runs out (about an hour). [Refresh] asks now. A copy is kept in \
             assets-cache.json, so the list shows right after Athena starts.",
        )
        .color(DIM),
    );
}
