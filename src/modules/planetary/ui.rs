//! Draws the Planetary panel from the mockup Kevin approved
//! (<https://claude.ai/artifact/M7hLkXTru753UwZWhhqPN6>): one card per
//! character, one row per planet with a coloured dot, its status and a bar
//! per launchpad, details on click, and a Settings view.
//!
//! Drawing only reads the module's data (statuses are worked out in
//! `Planetary::assess_all`, not here) and records clicks.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, RichText, Sense,
    Stroke, Vec2, text::LayoutJob, text::TextWrapping,
};

use super::Planetary;
use super::data::Colony;
use super::status::{Assessment, Level, Store, duration_text};
use crate::localtime;
use crate::static_data::PiStructureKind;

// Colours from the mockup (the same as Finance).
const GREEN: Color32 = Color32::from_rgb(0x7c, 0xc4, 0x7c);
const BAR_GREEN: Color32 = Color32::from_rgb(0x4f, 0x8f, 0x5f);
const RED: Color32 = Color32::from_rgb(0xef, 0x6b, 0x6b);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
const BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const TEXT: Color32 = Color32::from_rgb(0xc8, 0xc8, 0xc8);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
const CARD_FILL: Color32 = Color32::from_rgb(0x23, 0x23, 0x23);
const CARD_BORDER: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
const ROW_LINE: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x2f);
const ROW_HOVER: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
const CHIP_FILL: Color32 = Color32::from_rgb(0x2e, 0x3a, 0x44);
const CHIP_TEXT: Color32 = Color32::from_rgb(0xa9, 0xc4, 0xd8);
const BAR_BACK: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
const DETAIL_FILL: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
const DETAIL_BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);

/// Cards are at least this wide, and at most `CARD_MAX_WIDTH` (Kevin's
/// "at most 480 px per column").
const CARD_MIN_WIDTH: f32 = 440.0;
const CARD_MAX_WIDTH: f32 = 480.0;
const GAP: f32 = 8.0;
const ROW_HEIGHT: f32 = 40.0;
/// The launchpad bars together, and the percent text after them.
const BARS_WIDTH: f32 = 120.0;
const BAR_GAP: f32 = 3.0;
const PERCENT_WIDTH: f32 = 60.0;
const DETAIL_LABEL_WIDTH: f32 = 92.0;
/// Contents listed per launchpad in the details.
const MAX_CONTENTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Planets,
    Settings,
}

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

/// The panel.
pub fn show(module: &mut Planetary, ui: &mut egui::Ui, now: i64) {
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    view_tabs(module, ui, now);
    for problem in &module.problems {
        ui.colored_label(YELLOW, small(problem));
    }
    if module.view == View::Settings {
        settings_view(module, ui);
        return;
    }
    if !crate::auth::is_configured() {
        ui.colored_label(YELLOW, "EVE login isn't set up in this build.");
        return;
    }
    if module.characters.is_empty() {
        ui.label("Log in your characters on the Characters tab, and their planets show up here.");
        return;
    }
    if module.static_data.is_none() {
        return;
    }
    if !module.loaded {
        ui.label(small("Reading saved planet data…").color(DIM));
    }
    planets_view(module, ui, now);
}

fn view_tabs(module: &mut Planetary, ui: &mut egui::Ui, now: i64) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut module.view, View::Planets, "Planets");
        ui.selectable_value(&mut module.view, View::Settings, "Settings");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(module.can_refresh(now), egui::Button::new(small("Refresh")))
                .on_hover_text(
                    "Ask ESI for every colony now. ESI may still send its cached copy, and it \
                     only has news after you open a colony in EVE.",
                )
                .on_disabled_hover_text("Just refreshed. Try again in a minute.")
                .clicked()
            {
                module.refresh_now();
            }
            let text = match module.checked {
                Some(at) => format!("Asked ESI {} · every 10 min", ago(now - at)),
                None => "Asking ESI…".to_string(),
            };
            ui.label(small(text).color(DIM));
        });
    });
}

/// "just now", "4 min ago", "5 h ago", "2 d ago".
fn ago(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes} min ago")
    } else if minutes < 48 * 60 {
        format!("{} h ago", minutes / 60)
    } else {
        format!("{} d ago", minutes / (24 * 60))
    }
}

/// One character's card, ready to draw.
struct CharacterCard {
    id: u64,
    name: String,
    count: String,
    problem: Option<String>,
    /// Planets in the order to show, with their status.
    planets: Vec<(Colony, Assessment)>,
    attention: usize,
    soon: usize,
}

/// Builds the cards of the characters to show (tag filter applied).
fn cards(module: &Planetary) -> Vec<CharacterCard> {
    let mut out = Vec::new();
    for c in &module.characters {
        if let Some(tag) = &module.settings.tag
            && !c.tags.contains(tag)
        {
            continue;
        }
        let colonies = module.colonies.get(&c.id).cloned().unwrap_or_default();
        let mut planets: Vec<(Colony, Assessment)> = colonies
            .into_iter()
            .filter_map(|colony| {
                let a = module.assessed.get(&colony.planet_id)?.clone();
                Some((colony, a))
            })
            .collect();
        planets.sort_by(|a, b| {
            a.1.level
                .cmp(&b.1.level)
                .then_with(|| module.planet_name(&a.0).cmp(&module.planet_name(&b.0)))
        });
        let attention = planets
            .iter()
            .filter(|(_, a)| a.level == Level::Attention)
            .count();
        let soon = planets
            .iter()
            .filter(|(_, a)| a.level == Level::Soon)
            .count();
        let count = match module.max_planets.get(&c.id) {
            Some(max) => format!("{} of {max} planets", planets.len()),
            None => format!("{} planets", planets.len()),
        };
        out.push(CharacterCard {
            id: c.id,
            name: c.name.clone(),
            count,
            problem: module.character_problems.get(&c.id).cloned(),
            planets,
            attention,
            soon,
        });
    }
    out
}

fn planets_view(module: &mut Planetary, ui: &mut egui::Ui, now: i64) {
    let mut cards = cards(module);
    let planets: usize = cards.iter().map(|c| c.planets.len()).sum();
    let attention: usize = cards.iter().map(|c| c.attention).sum();
    let soon: usize = cards.iter().map(|c| c.soon).sum();
    let active = cards
        .iter()
        .flat_map(|c| c.planets.iter())
        .filter(|(_, a)| a.level == Level::Active)
        .count();

    // Filters.
    let mut tags: Vec<String> = module
        .characters
        .iter()
        .flat_map(|c| c.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        let only = &mut module.settings.only_attention;
        changed |= ui
            .selectable_value(only, false, small("All planets"))
            .changed();
        changed |= ui
            .selectable_value(
                only,
                true,
                small(format!("Needs attention ({})", attention + soon)),
            )
            .on_hover_text("Hide the planets that are fine")
            .changed();
        if !tags.is_empty() {
            ui.add_space(8.0);
            ui.label(small("Tag").color(DIM));
            for tag in &tags {
                let picked = module.settings.tag.as_ref() == Some(tag);
                if ui
                    .add(egui::Button::selectable(picked, small(tag)).corner_radius(8))
                    .on_hover_text("Only show characters with this tag (click again for all)")
                    .clicked()
                {
                    module.settings.tag = if picked { None } else { Some(tag.clone()) };
                    changed = true;
                }
            }
        }
    });
    if changed {
        module.save_settings();
    }

    // Totals.
    ui.horizontal_wrapped(|ui| {
        ui.label(small(format!("{} characters · {planets} planets", cards.len())).color(DIM));
        dot_label(ui, RED, format!("{attention} need attention"));
        dot_label(ui, YELLOW, format!("{soon} soon"));
        dot_label(ui, GREEN, format!("{active} active"));
    });
    ui.label(
        small(
            "ESI only updates a colony when you open it in EVE, so every planet shows how old \
             its data is.",
        )
        .color(DIM),
    );

    if module.settings.only_attention {
        for card in &mut cards {
            card.planets
                .retain(|(_, a)| matches!(a.level, Level::Attention | Level::Soon));
        }
        cards.retain(|c| !c.planets.is_empty());
        if cards.is_empty() {
            ui.label("Nothing needs attention.");
            return;
        }
    }

    let thresholds = module.settings.thresholds.clone();
    let mut toggled = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available = ui.available_width();
            let count = (((available + GAP) / (CARD_MIN_WIDTH + GAP)).floor() as usize)
                .clamp(1, cards.len().max(1));
            let total = (CARD_MAX_WIDTH * count as f32 + GAP * (count as f32 - 1.0)).min(available);
            ui.set_max_width(total);
            // `columns` splits the width; cards go left to right, then the
            // next row, so the Characters tab's order reads across.
            ui.columns(count, |columns| {
                for (index, card) in cards.iter().enumerate() {
                    let column = &mut columns[index % count];
                    column.spacing_mut().item_spacing = Vec2::new(6.0, GAP);
                    if let Some(id) = character_card(module, column, card, &thresholds, now) {
                        toggled = Some(id);
                    }
                }
            });
        });
    if let Some(planet_id) = toggled
        && !module.open.remove(&planet_id)
    {
        module.open.insert(planet_id);
    }
}

/// Draws one card. Returns the planet whose row was clicked.
fn character_card(
    module: &Planetary,
    ui: &mut egui::Ui,
    card: &CharacterCard,
    thresholds: &super::config::Thresholds,
    now: i64,
) -> Option<i64> {
    let mut clicked = None;
    egui::Frame::new()
        .fill(CARD_FILL)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&card.name).size(14.0).color(BRIGHT));
                ui.label(small(&card.count).color(DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (badge, color) = if card.attention > 0 {
                        (format!("{} need attention", card.attention), RED)
                    } else if card.soon > 0 {
                        (format!("{} soon", card.soon), YELLOW)
                    } else if card.planets.is_empty() {
                        (String::new(), DIM)
                    } else {
                        ("All active".to_string(), GREEN)
                    };
                    ui.label(small(badge).color(color));
                    if let Some(problem) = &card.problem {
                        let note = if problem.starts_with("Login expired") {
                            "Login expired"
                        } else {
                            "ESI problem"
                        };
                        ui.label(small(note).color(YELLOW)).on_hover_text(problem);
                    }
                });
            });
            ui.add_space(4.0);
            if card.planets.is_empty() {
                let text = if module.colonies.contains_key(&card.id) {
                    "No colonies."
                } else {
                    "Waiting for ESI…"
                };
                ui.label(small(text).color(DIM));
            }
            for (colony, assessment) in &card.planets {
                if planet_row(module, ui, colony, assessment, thresholds, now) {
                    clicked = Some(colony.planet_id);
                }
                if module.open.contains(&colony.planet_id) {
                    planet_details(module, ui, colony, assessment, now);
                }
            }
        });
    clicked
}

/// A small coloured dot and a dim label after it (the font has no "●").
fn dot_label(ui: &mut egui::Ui, color: Color32, text: String) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 14.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, color);
        ui.label(small(text).color(DIM));
    });
}

fn level_color(level: Level) -> Color32 {
    match level {
        Level::Attention => RED,
        Level::Soon => YELLOW,
        Level::Active => GREEN,
        Level::Unknown => DIM,
    }
}

/// A bar's colour by how full it is.
fn fill_color(percent: f64, thresholds: &super::config::Thresholds) -> Color32 {
    if percent >= thresholds.full_percent as f64 {
        RED
    } else if percent >= thresholds.warn_percent as f64 {
        YELLOW
    } else {
        BAR_GREEN
    }
}

/// One line of text that ends in "…" if it doesn't fit in `width`.
fn one_line(
    painter: &egui::Painter,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), FontId::proportional(size), color);
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

/// Draws a planet's row. Returns true when it was clicked.
fn planet_row(
    module: &Planetary,
    ui: &mut egui::Ui,
    colony: &Colony,
    a: &Assessment,
    thresholds: &super::config::Thresholds,
    now: i64,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 2.0, ROW_HOVER);
    }
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, ROW_LINE));

    let left = rect.left() + 15.0;
    let top = rect.top() + 5.0;
    painter.circle_filled(
        Pos2::new(rect.left() + 5.0, top + 8.0),
        4.0,
        level_color(a.level),
    );

    // Line 1: name, type, kind chip ... viewed x ago.
    let age = now - colony.last_update;
    let age_color = if age > thresholds.old_days as i64 * 86_400 {
        YELLOW
    } else {
        DIM
    };
    let age_galley = painter.layout_no_wrap(
        format!("Viewed {}", ago(age)),
        FontId::proportional(12.0),
        age_color,
    );
    let age_x = rect.right() - age_galley.size().x;
    painter.galley(Pos2::new(age_x, top + 1.0), age_galley, age_color);
    let name = module.planet_name(colony);
    let name_galley = one_line(
        &painter,
        &name,
        13.5,
        BRIGHT,
        (age_x - left - 150.0).max(60.0),
    );
    let mut x = left;
    let name_width = name_galley.size().x;
    painter.galley(Pos2::new(x, top), name_galley, BRIGHT);
    x += name_width + 6.0;
    let type_galley = painter.layout_no_wrap(colony.type_label(), FontId::proportional(12.0), DIM);
    let type_width = type_galley.size().x;
    if x + type_width < age_x - 8.0 {
        painter.galley(Pos2::new(x, top + 1.5), type_galley, DIM);
        x += type_width + 6.0;
    }
    if a.level != Level::Unknown {
        let chip = painter.layout_no_wrap(
            a.kind.label().to_string(),
            FontId::proportional(11.0),
            CHIP_TEXT,
        );
        let chip_rect = Rect::from_min_size(
            Pos2::new(x, top + 1.0),
            Vec2::new(chip.size().x + 10.0, chip.size().y + 2.0),
        );
        if chip_rect.right() < age_x - 8.0 {
            painter.rect_filled(chip_rect, 3.0, CHIP_FILL);
            painter.galley(chip_rect.min + Vec2::new(5.0, 1.0), chip, CHIP_TEXT);
        }
    }

    // Line 2: status ... pad bars and percent.
    let line2 = rect.top() + 23.0;
    let stores = a.bar_stores();
    let mut status_right = rect.right();
    if !stores.is_empty() {
        let percent_left = rect.right() - PERCENT_WIDTH;
        let bars_left = percent_left - BARS_WIDTH - 6.0;
        draw_bars(
            &painter,
            stores,
            Rect::from_min_size(
                Pos2::new(bars_left, line2 + 2.0),
                Vec2::new(BARS_WIDTH, 10.0),
            ),
            thresholds,
        );
        let worst = stores.iter().map(Store::percent).fold(0.0, f64::max);
        // The hatched part of the bars shows what is estimated; the
        // details say it in words.
        let mut text = String::new();
        if stores.len() > 1 {
            text.push_str("max ");
        }
        text.push_str(&format!("{worst:.0}%"));
        let percent = painter.layout_no_wrap(text, FontId::proportional(12.0), TEXT);
        let px = rect.right() - percent.size().x;
        painter.galley(Pos2::new(px, line2), percent, TEXT);
        let label = if stores.len() > 1 {
            format!("{} pads", stores.len())
        } else if stores[0].kind == PiStructureKind::Launchpad {
            "Pad".to_string()
        } else {
            "Storage".to_string()
        };
        let label = painter.layout_no_wrap(label, FontId::proportional(12.0), DIM);
        let lx = bars_left - 6.0 - label.size().x;
        painter.galley(Pos2::new(lx, line2), label, DIM);
        status_right = lx - 8.0;
    }
    let status_color = match a.level {
        Level::Active => TEXT,
        level => level_color(level),
    };
    let status = one_line(
        &painter,
        &a.headline,
        12.5,
        status_color,
        status_right - left,
    );
    painter.galley(Pos2::new(left, line2 - 0.5), status, status_color);

    response.on_hover_text("Click for details").clicked()
}

/// One bar per launchpad side by side: solid = at the last view, hatched
/// = estimated since then.
fn draw_bars(
    painter: &egui::Painter,
    stores: &[Store],
    area: Rect,
    thresholds: &super::config::Thresholds,
) {
    let n = stores.len() as f32;
    let width = ((area.width() - BAR_GAP * (n - 1.0)) / n).max(4.0);
    for (i, store) in stores.iter().enumerate() {
        let x = area.left() + i as f32 * (width + BAR_GAP);
        let bar = Rect::from_min_size(Pos2::new(x, area.top()), Vec2::new(width, area.height()));
        painter.rect_filled(bar, 1.0, BAR_BACK);
        let color = fill_color(store.percent(), thresholds);
        let solid = (store.percent_at_view() / 100.0) as f32 * width;
        let total = (store.percent() / 100.0) as f32 * width;
        if solid > 0.0 {
            painter.rect_filled(
                Rect::from_min_size(bar.min, Vec2::new(solid, bar.height())),
                1.0,
                color,
            );
        }
        if total > solid + 0.5 {
            let hatch = Rect::from_min_max(
                Pos2::new(bar.left() + solid, bar.top()),
                Pos2::new(bar.left() + total, bar.bottom()),
            );
            let clipped = painter.with_clip_rect(hatch);
            let mut sx = hatch.left() - hatch.height();
            while sx < hatch.right() {
                clipped.line_segment(
                    [
                        Pos2::new(sx, hatch.bottom()),
                        Pos2::new(sx + hatch.height(), hatch.top()),
                    ],
                    Stroke::new(1.5, color),
                );
                sx += 4.0;
            }
        }
        painter.rect_stroke(
            bar,
            1.0,
            Stroke::new(1.0, CARD_BORDER),
            egui::StrokeKind::Inside,
        );
    }
}

/// "Thu 12 Nov 14:20" in local time.
fn when_text(unix: i64) -> String {
    let date = localtime::date_text(unix);
    // Drop the year: "Thu 12 Nov 2026" -> "Thu 12 Nov".
    let date = date.rsplit_once(' ').map_or(date.as_str(), |(d, _)| d);
    let clock = localtime::clock_text(unix);
    format!("{date} {}", &clock[..5])
}

/// "1,234,567".
fn grouped(value: f64) -> String {
    let digits = (value.round().max(0.0) as u64).to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The open details under a planet's row.
fn planet_details(
    module: &Planetary,
    ui: &mut egui::Ui,
    colony: &Colony,
    a: &Assessment,
    now: i64,
) {
    let Some(data) = module.static_data.as_deref() else {
        return;
    };
    let name_of = |type_id: i64| {
        data.pi_commodity(type_id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("#{type_id}"))
    };
    let soon_secs = module.settings.thresholds.extractor_soon_hours as i64 * 3600;

    // (label, value, right-hand note, note colour)
    let mut rows: Vec<(String, String, String, Color32)> = Vec::new();
    for e in &a.extractors {
        match (e.product, e.expiry) {
            (Some(product), Some(expiry)) => {
                let (note, color) = if expiry <= now {
                    (format!("stopped {}", when_text(expiry)), RED)
                } else if expiry - now < soon_secs {
                    (format!("ends {}", when_text(expiry)), YELLOW)
                } else {
                    (format!("ends {}", when_text(expiry)), TEXT)
                };
                rows.push((
                    "Extractor".into(),
                    format!(
                        "{} · {}/h · {} heads",
                        name_of(product),
                        grouped(e.per_hour),
                        e.heads
                    ),
                    note,
                    color,
                ));
            }
            _ => rows.push((
                "Extractor".into(),
                "No program installed".into(),
                String::new(),
                RED,
            )),
        }
    }
    for f in &a.factories {
        let what = f
            .schematic_id
            .and_then(|id| data.pi_schematic(id))
            .map_or_else(|| "no schematic".to_string(), |s| s.name.clone());
        let (note, color) = if f.idle > 0 {
            (format!("{} idle at last view", f.idle), YELLOW)
        } else {
            ("running".to_string(), GREEN)
        };
        rows.push((
            "Factories".into(),
            format!("{} × {what}", f.count),
            note,
            color,
        ));
    }
    if let Some(left) = a.inputs_left {
        let names: Vec<String> = a.inputs.iter().map(|id| name_of(*id)).collect();
        let (note, color) = if left <= 0 {
            (format!("ran out ~{} ago", duration_text(-left)), RED)
        } else {
            (format!("last ~{}", duration_text(left)), TEXT)
        };
        rows.push(("Inputs".into(), names.join(", "), note, color));
    }
    let numbered = |list: &[Store], base: &str| -> Vec<(String, String, String, Color32)> {
        list.iter()
            .enumerate()
            .map(|(i, s)| {
                let label = if list.len() > 1 {
                    format!("{base} {}", i + 1)
                } else {
                    base.to_string()
                };
                let mut value = format!("{} / {} m³", grouped(s.used_m3), grouped(s.capacity_m3));
                let contents: Vec<String> = s
                    .contents
                    .iter()
                    .take(MAX_CONTENTS)
                    .map(|(id, n)| format!("{} {}", name_of(*id), grouped(*n as f64)))
                    .collect();
                if !contents.is_empty() {
                    value.push_str(": ");
                    value.push_str(&contents.join(", "));
                    if s.contents.len() > MAX_CONTENTS {
                        value.push_str(", …");
                    }
                }
                let mut note = format!("{:.0}%", s.percent_at_view());
                if s.added_m3 > 0.0 {
                    note = format!(
                        "{note}, ~{:.0}% now (+{} m³ est.)",
                        s.percent(),
                        grouped(s.added_m3)
                    );
                }
                let color = fill_color(s.percent(), &module.settings.thresholds);
                let color = if color == BAR_GREEN { TEXT } else { color };
                (label, value, note, color)
            })
            .collect()
    };
    rows.extend(numbered(&a.launchpads, "Launchpad"));
    rows.extend(numbered(&a.storages, "Storage"));

    egui::Frame::new()
        .fill(DETAIL_FILL)
        .stroke(Stroke::new(1.0, DETAIL_BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(8, 6))
        .outer_margin(Margin {
            left: 15,
            right: 0,
            top: 0,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 3.0);
            for (label, value, note, color) in rows {
                // The label column, then the value (and note) wrapping in
                // the width that's left.
                ui.horizontal_top(|ui| {
                    ui.add_sized(
                        [DETAIL_LABEL_WIDTH, 14.0],
                        egui::Label::new(small(label).color(DIM)),
                    );
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(small(value).color(TEXT));
                            if !note.is_empty() {
                                ui.label(small(note).color(color));
                            }
                        });
                    });
                });
            }
            ui.label(
                small(format!(
                    "In-game data from {}. Open the colony in EVE to update it.",
                    when_text(colony.last_update)
                ))
                .color(DIM),
            );
        });
}

// ---------------------------------------------------------------- Settings

fn settings_view(module: &mut Planetary, ui: &mut egui::Ui) {
    // A child area, so the width limit applies to the rows' right edge.
    let width = CARD_MAX_WIDTH.min(ui.available_width());
    ui.allocate_ui(Vec2::new(width, ui.available_height()), |ui| {
        ui.set_max_width(width);
        settings_rows(module, ui);
    });
}

fn settings_rows(module: &mut Planetary, ui: &mut egui::Ui) {
    let t = &mut module.settings.thresholds;
    let before = t.clone();
    let row = |ui: &mut egui::Ui,
               text: &str,
               value: &mut u32,
               range: std::ops::RangeInclusive<u32>,
               unit: &str| {
        ui.horizontal(|ui| {
            ui.label(text);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(unit);
                ui.add(egui::DragValue::new(value).range(range).speed(0.2));
            });
        });
    };
    ui.heading("Planetary");
    row(
        ui,
        "Extractors count as \"soon\" when they end within",
        &mut t.extractor_soon_hours,
        1..=168,
        "h",
    );
    row(
        ui,
        "Factory inputs count as \"soon\" when they last less than",
        &mut t.factory_soon_hours,
        1..=168,
        "h",
    );
    row(
        ui,
        "Launchpad / storage yellow at",
        &mut t.warn_percent,
        1..=100,
        "%",
    );
    row(
        ui,
        "Launchpad / storage red (\"full\") at",
        &mut t.full_percent,
        1..=100,
        "%",
    );
    row(
        ui,
        "Colony data shows in yellow when older than",
        &mut t.old_days,
        1..=60,
        "days",
    );
    ui.checkbox(
        &mut t.show_estimate,
        "Show estimated launchpad fill since the last in-game view (hatched)",
    );
    if t.full_percent < t.warn_percent {
        t.full_percent = t.warn_percent;
    }
    let changed = *t != before;
    ui.add_space(8.0);
    ui.label(
        small(
            "Athena asks ESI every 10 minutes (ESI's own cache time) and only loads a planet's \
             layout again when EVE reports a newer view. The estimate uses each extractor's \
             starting rate, so it is rough. No sound or popup alerts in this version.",
        )
        .color(DIM),
    );
    if ui.button("Reset to defaults").clicked() {
        module.settings.thresholds = super::config::Thresholds::default();
        module.save_settings();
    } else if changed {
        module.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_texts() {
        assert_eq!(ago(30), "just now");
        assert_eq!(ago(5 * 60), "5 min ago");
        assert_eq!(ago(5 * 3600), "5 h ago");
        assert_eq!(ago(3 * 86_400), "3 d ago");
    }

    #[test]
    fn grouped_numbers() {
        assert_eq!(grouped(0.0), "0");
        assert_eq!(grouped(999.4), "999");
        assert_eq!(grouped(1_840.0), "1,840");
        assert_eq!(grouped(412_000.0), "412,000");
    }
}
