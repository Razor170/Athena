//! The "Open ship" window: stations with ships on the left, the ships of
//! the picked station on the right, and how old each character's ESI data
//! is at the bottom. Follows the approved mockup
//! (<https://claude.ai/artifact/WCWHiD42SuZELRHgsxXw18>).

use std::collections::BTreeMap;
use std::time::Duration;

use eframe::egui::{self, Color32, FontId, RichText, text::LayoutJob, text::TextFormat, vec2};

use super::Fittings;
use super::assets::{Place, Ship};
use super::ui::{ACCENT, ago_text, hour_minute};
use super::{Side, Who};
use crate::auth::now_unix;

const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
const DIM: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
const YELLOW: Color32 = Color32::from_rgb(0xe0, 0xb0, 0x40);
/// A pane is at most this wide (Athena's 480 px rule).
const MAX_PANE: f32 = 480.0;
const PANE_HEIGHT: f32 = 360.0;

/// One ship as listed: whose, which, and its name.
struct Listed<'a> {
    character_id: u64,
    ship: &'a Ship,
    name: &'a str,
}

/// What a click in the window asks for, done after drawing.
#[derive(Default)]
struct Clicks {
    load: Option<(u64, i64)>,
    refresh: bool,
}

/// Draws the window while it is open.
pub fn show(module: &mut Fittings, ctx: &egui::Context) {
    if !module.picker.open {
        return;
    }
    let title = match module.picker.target {
        Side::Is => "Open ship into Is",
        Side::Should => "Open ship into Should",
    };
    let mut open = true;
    let mut clicks = Clicks::default();
    egui::Window::new(title)
        .id(egui::Id::new("fittings_open_ship"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(vec2(2.0 * MAX_PANE + 24.0, PANE_HEIGHT + 120.0))
        .show(ctx, |ui| body(module, ui, &mut clicks));
    if !open {
        module.picker.open = false;
    }
    if let Some((character_id, item_id)) = clicks.load {
        module.load_ship(character_id, item_id);
    } else if clicks.refresh {
        module.fetch_assets(ctx);
    }
    if module.picker.open {
        // "23 min ago" and the Refresh button change with the clock: one
        // repaint a minute while the window is open, none once it closes.
        ctx.request_repaint_after(Duration::from_secs(60));
    }
}

fn small(text: impl Into<String>) -> RichText {
    RichText::new(text).size(12.0)
}

fn body(module: &mut Fittings, ui: &mut egui::Ui, clicks: &mut Clicks) {
    if module.static_data.is_none() {
        ui.label(
            small("Athena needs its game data file (static.sqlite) to find ships.").color(YELLOW),
        );
        return;
    }
    if module.characters.is_empty() {
        ui.label(
            small("No characters yet. Log in on the Characters tab, then try again.").color(DIM),
        );
        return;
    }

    // Search and character filter.
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut module.picker.search)
                .hint_text("Search station, ship or type")
                .desired_width(240.0),
        );
        let selected = match &module.picker.who {
            Who::All => "All characters".to_string(),
            Who::Character(id) => module.character_name(*id),
            Who::Tag(tag) => format!("Tag: {tag}"),
        };
        let mut tags: Vec<String> = module
            .characters
            .iter()
            .flat_map(|c| c.tags.iter().cloned())
            .collect();
        tags.sort();
        tags.dedup();
        egui::ComboBox::from_id_salt("fittings_open_ship_character")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                let who = &mut module.picker.who;
                ui.selectable_value(who, Who::All, "All characters");
                for tag in tags {
                    let label = format!("Tag: {tag}");
                    ui.selectable_value(who, Who::Tag(tag), label);
                }
                for c in &module.characters {
                    ui.selectable_value(who, Who::Character(c.id), &c.name);
                }
            });
    });
    ui.separator();

    // The ships that pass the filters, by place.
    let search = module.picker.search.trim().to_lowercase();
    let mut by_place: BTreeMap<Place, Vec<Listed>> = BTreeMap::new();
    for (character_id, entry) in &module.assets {
        if !module.picker_lists(*character_id) {
            continue;
        }
        for (ship, name) in &entry.ships {
            if !search.is_empty() {
                let hull = module.hull_name(ship.type_id).to_lowercase();
                let place = module.place_name(ship.place).to_lowercase();
                if !name.to_lowercase().contains(&search)
                    && !hull.contains(&search)
                    && !place.contains(&search)
                {
                    continue;
                }
            }
            by_place.entry(ship.place).or_default().push(Listed {
                character_id: *character_id,
                ship,
                name,
            });
        }
    }
    // Most ships first, then by name.
    let mut places: Vec<(Place, String)> = by_place
        .keys()
        .map(|p| (*p, module.place_name(*p)))
        .collect();
    places.sort_by(|a, b| {
        by_place[&b.0]
            .len()
            .cmp(&by_place[&a.0].len())
            .then_with(|| a.1.cmp(&b.1))
    });
    // `by_place` borrows from `module.assets`, so the picked place is kept
    // in a local while drawing and written back to `module` at the end.
    let mut picked = module.picker.place;
    if picked.is_none_or(|p| !by_place.contains_key(&p)) {
        picked = places.first().map(|(p, _)| *p);
    }

    let pane = ((ui.available_width() - 12.0) / 2.0).clamp(200.0, MAX_PANE);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        // Stations.
        ui.vertical(|ui| {
            ui.set_width(pane);
            ui.label(small("STATIONS WITH SHIPS").size(10.5).color(DIM));
            egui::ScrollArea::vertical()
                .id_salt("fittings_places")
                .max_height(PANE_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if places.is_empty() {
                        let any_loading = module.assets.values().any(|a| a.loading);
                        let text = if any_loading {
                            "Loading assets from ESI…"
                        } else {
                            "No ships found."
                        };
                        ui.label(small(text).italics().color(DIM));
                    }
                    for (place, name) in &places {
                        let ships = &by_place[place];
                        let mut owners: Vec<String> = ships
                            .iter()
                            .map(|l| module.character_name(l.character_id))
                            .collect();
                        owners.sort();
                        owners.dedup();
                        let count = if ships.len() == 1 {
                            "1 ship".to_string()
                        } else {
                            format!("{} ships", ships.len())
                        };
                        let job = two_lines(name, &format!("{count} · {}", owners.join(", ")));
                        let selected = picked == Some(*place);
                        // Full pane width. The text follows the layout's
                        // alignment (left here); `add_sized` would center it.
                        let button =
                            egui::Button::selectable(selected, job).min_size(vec2(pane, 0.0));
                        if ui.add(button).clicked() {
                            picked = Some(*place);
                        }
                    }
                });
        });
        // Ships at the picked place.
        ui.vertical(|ui| {
            ui.set_width(pane);
            ui.label(small("SHIPS").size(10.5).color(DIM));
            egui::ScrollArea::vertical()
                .id_salt("fittings_ships")
                .max_height(PANE_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let Some(ships) = picked.and_then(|p| by_place.get(&p)) else {
                        return;
                    };
                    for listed in ships {
                        ship_row(module, ui, listed, clicks);
                    }
                });
        });
    });

    module.picker.place = picked;

    ui.separator();
    footer(module, ui, clicks);
}

/// One ship: type and name, what is in it, and Load.
fn ship_row(module: &Fittings, ui: &mut egui::Ui, listed: &Listed, clicks: &mut Clicks) {
    let hull = module.hull_name(listed.ship.type_id);
    let mut second = format!(
        "{} · {}",
        listed.ship.summary(),
        module.character_name(listed.character_id)
    );
    if let Some(carrier) = listed.ship.inside {
        let carrier_name = module
            .assets
            .get(&listed.character_id)
            .and_then(|a| a.ships.iter().find(|(s, _)| s.item_id == carrier))
            .map_or("another ship", |(_, n)| n.as_str());
        second.push_str(&format!(" · in {carrier_name}"));
    }
    ui.horizontal(|ui| {
        // The two text lines take the width left of the Load button, each
        // cut with "…" when too long. (One label with both lines can't be
        // truncated: truncating keeps only its first line.)
        let text_width = (ui.available_width() - 56.0).max(80.0);
        ui.allocate_ui_with_layout(
            vec2(text_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(text_width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let first = format!("{hull} · {}", listed.name);
                ui.add(egui::Label::new(RichText::new(first).size(13.0).color(TEXT)).truncate());
                ui.add(egui::Label::new(small(second).size(11.5).color(DIM)).truncate());
            },
        );
        if ui.button("Load").clicked() {
            clicks.load = Some((listed.character_id, listed.ship.item_id));
        }
    });
    ui.add_space(2.0);
}

/// Per character: when ESI made the data and when it has more. Plus
/// Refresh, greyed out until some character's data can be newer.
fn footer(module: &Fittings, ui: &mut egui::Ui, clicks: &mut Clicks) {
    let now = now_unix();
    let fetched: Vec<i64> = module
        .characters
        .iter()
        .filter_map(|c| module.assets.get(&c.id))
        .filter(|a| !a.loading)
        .map(|a| a.next_at)
        .collect();
    let any_loading = module.assets.values().any(|a| a.loading);
    let due = fetched.iter().copied().min();
    ui.horizontal(|ui| {
        ui.label(small(format!("ESI assets, now {}", hour_minute(now))).color(DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (enabled, label) = match due {
                _ if any_loading => (false, "Loading…".to_string()),
                Some(at) if at > now => (false, format!("Refresh at {}", hour_minute(at))),
                _ => (true, "Refresh".to_string()),
            };
            if ui
                .add_enabled(enabled, egui::Button::new(small(label)))
                .on_disabled_hover_text("ESI has no newer assets yet.")
                .clicked()
            {
                clicks.refresh = true;
            }
        });
    });
    for character in &module.characters {
        let entry = module.assets.get(&character.id);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(small(&character.name).color(ACCENT));
            match entry {
                None => {
                    ui.label(small("not fetched yet").color(DIM));
                }
                // A plain label, not a spinner: a spinner repaints every
                // frame, and a fetch can take a while.
                Some(a) if a.loading => {
                    ui.label(small("loading…").color(DIM));
                }
                Some(a) => {
                    if let Some(from) = a.data_from {
                        let age = now - from;
                        let color = if age > 3600 { YELLOW } else { DIM };
                        ui.label(
                            small(format!(
                                "data from {} ({})",
                                hour_minute(from),
                                ago_text(age)
                            ))
                            .color(color),
                        );
                    }
                    match &a.problem {
                        Some(problem) => {
                            ui.label(
                                small(format!("· {problem} · retry {}", hour_minute(a.next_at)))
                                    .color(YELLOW),
                            );
                        }
                        None => {
                            ui.label(
                                small(format!("· next {}", hour_minute(a.next_at))).color(DIM),
                            );
                        }
                    }
                }
            }
        });
    }
}

/// A bright first line and a dim, smaller second line.
fn two_lines(first: &str, second: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        first,
        0.0,
        TextFormat::simple(FontId::proportional(13.0), TEXT),
    );
    job.append(
        &format!("\n{second}"),
        0.0,
        TextFormat::simple(FontId::proportional(11.5), DIM),
    );
    job
}
